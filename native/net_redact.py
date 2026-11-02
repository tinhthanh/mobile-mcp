"""Redaction used by net_addon.py, kept free of mitmproxy imports so it can be tested alone."""
import re

REDACT_HEADERS = {"authorization", "cookie", "set-cookie", "x-api-key", "proxy-authorization"}

# secrets inside bodies: json/form values under credential-like keys, and bearer tokens
SECRET_KEYS = r"[\w-]*(?:token|password|passwd|secret|authorization|api[_-]?key|cookie|session)[\w-]*"
SECRET_JSON = re.compile(r'("' + SECRET_KEYS + r'"\s*:\s*)"(?:[^"\\]|\\.)*"', re.IGNORECASE)
# the same json escaped inside a string value, e.g. an echoed request body
SECRET_ESCAPED = re.compile(r'(\\"' + SECRET_KEYS + r'\\"\s*:\s*\\")[^"\\]*', re.IGNORECASE)
SECRET_FORM = re.compile(r"((?:^|&)" + SECRET_KEYS + r"=)[^&]*", re.IGNORECASE)
BEARER = re.compile(r"(Bearer\s+)[A-Za-z0-9._~+/=-]+", re.IGNORECASE)


def redact(text):
    text = SECRET_JSON.sub(r'\1"[redacted]"', text)
    text = SECRET_ESCAPED.sub(r"\1[redacted]", text)
    text = SECRET_FORM.sub(r"\1[redacted]", text)
    return BEARER.sub(r"\1[redacted]", text)
