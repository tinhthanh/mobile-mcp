"""mitmdump addon for mobile-mcp-lite's `net` tool.

Writes one JSON line per request to $NET_LOG and request/response details
(redacted headers, truncated bodies) to $NET_DETAIL.

TLS is passed through untouched unless the SNI matches $NET_DECRYPT (a regex):
decrypting for an app that does not trust the mitmproxy CA would break its
connections, while passthrough still records which hosts it talks to.
"""
import json
import os
import re
import threading
import time

from mitmproxy import http, tls

MAX_BODY = 2000
REDACT = {"authorization", "cookie", "set-cookie", "x-api-key", "proxy-authorization"}
TEXT_TYPES = ("json", "text", "xml", "javascript", "x-www-form-urlencoded", "graphql")

# secrets inside bodies: json/form values under credential-like keys, and bearer tokens
SECRET_KEYS = r"[\w-]*(?:token|password|passwd|secret|authorization|api[_-]?key|cookie|session)[\w-]*"
SECRET_JSON = re.compile(r'("' + SECRET_KEYS + r'"\s*:\s*)"(?:[^"\\]|\\.)*"', re.IGNORECASE)
# the same json escaped inside a string value, e.g. an echoed request body
SECRET_ESCAPED = re.compile(r'(\\"' + SECRET_KEYS + r'\\"\s*:\s*\\")[^"\\]*', re.IGNORECASE)
SECRET_FORM = re.compile(r"((?:^|&)" + SECRET_KEYS + r"=)[^&]*", re.IGNORECASE)
BEARER = re.compile(r"(Bearer\s+)[A-Za-z0-9._~+/=-]+", re.IGNORECASE)

_lock = threading.Lock()
_next_id = 0
_log = open(os.environ["NET_LOG"], "a", buffering=1)
_detail = open(os.environ["NET_DETAIL"], "a", buffering=1)
_decrypt = re.compile(os.environ["NET_DECRYPT"]) if os.environ.get("NET_DECRYPT") else None


def _write(summary, detail=None):
    global _next_id
    with _lock:
        _next_id += 1
        summary["id"] = _next_id
        _log.write(json.dumps(summary, ensure_ascii=False) + "\n")
        if detail is not None:
            detail["id"] = _next_id
            _detail.write(json.dumps(detail, ensure_ascii=False) + "\n")


def _headers(headers):
    return {k: ("[redacted]" if k.lower() in REDACT else v) for k, v in headers.items()}


def _body(message):
    if message is None or not message.raw_content:
        return ""
    ctype = message.headers.get("content-type", "")
    size = len(message.raw_content)
    if not any(t in ctype for t in TEXT_TYPES):
        return f"<{ctype or 'binary'} {size} bytes>"
    text = message.get_text(strict=False) or ""
    text = SECRET_JSON.sub(r'\1"[redacted]"', text)
    text = SECRET_ESCAPED.sub(r"\1[redacted]", text)
    text = SECRET_FORM.sub(r"\1[redacted]", text)
    text = BEARER.sub(r"\1[redacted]", text)
    return text if len(text) <= MAX_BODY else text[:MAX_BODY] + f"…<{size} bytes total>"


def tls_clienthello(data: tls.ClientHelloData):
    sni = data.client_hello.sni or ""
    if _decrypt is not None and sni and _decrypt.search(sni):
        return
    data.ignore_connection = True
    _write({"t": round(time.time(), 3), "tls": True, "h": sni or "?"})


def response(flow: http.HTTPFlow):
    req, resp = flow.request, flow.response
    started = req.timestamp_start or time.time()
    ended = resp.timestamp_end or time.time()
    _write(
        {
            "t": round(started, 3),
            "m": req.method,
            "s": resp.status_code,
            "h": req.pretty_host,
            "p": req.path,
            "q": len(req.raw_content or b""),
            "r": len(resp.raw_content or b""),
            "ms": int((ended - started) * 1000),
            "ct": resp.headers.get("content-type", "").split(";")[0],
        },
        {
            "url": req.pretty_url,
            "req_headers": _headers(req.headers),
            "req_body": _body(req),
            "resp_headers": _headers(resp.headers),
            "resp_body": _body(resp),
        },
    )


def error(flow: http.HTTPFlow):
    req = flow.request
    _write(
        {
            "t": round(req.timestamp_start or time.time(), 3),
            "m": req.method,
            "s": 0,
            "h": req.pretty_host,
            "p": req.path,
            "err": str(flow.error)[:120],
        },
        {"url": req.pretty_url, "req_headers": _headers(req.headers), "req_body": _body(req), "error": str(flow.error)},
    )
