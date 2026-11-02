# Setup mobile-mcp-lite cho một máy Android mới

Hướng dẫn cài để Claude Code (hoặc agent khác qua MCP) điều khiển và bắt network của một điện thoại Android qua USB.
Đã kiểm chứng trên ZTE Blade A54, Android 13, không root.

## Một lệnh

```bash
git clone https://github.com/tinhthanh/mobile-mcp.git && mobile-mcp/scripts/setup.sh
```

Trên điện thoại chỉ cần: bật **Tùy chọn nhà phát triển** (bấm 7 lần vào *Số hiệu bản dựng*), bật **Gỡ lỗi USB**, cắm cáp,
bấm **Cho phép** ở hộp thoại *Allow USB debugging?* (tick *Luôn cho phép*). Chưa làm cũng được: script chờ tới 5 phút và
nói cần làm gì (chưa cắm máy, `unauthorized`, `offline`).

Lệnh này làm hết, chạy lại bao nhiêu lần cũng được (bước nào xong thì bỏ qua, ~6 s khi không có gì thay đổi):

1. **Mac**: cài adb, Node nếu thiếu; `npm ci` và build chỉ khi mã nguồn hoặc `package-lock.json` đổi.
2. **Mọi điện thoại đang cắm**: cài UiAutomator2 10.6.6 (APK ghim SHA-256), rồi khởi động thật và đọc một màn hình để chắc chắn chạy được.
3. **Claude Code**: đăng ký MCP `mobile-lite`, subagent `android-tester`, và (hỏi trước) thêm quyền `mcp__mobile-lite` vào `~/.claude/settings.json` để agent không hỏi từng thao tác.
4. **Tóm tắt** từng máy và một câu để thử.

| Tuỳ chọn | Tác dụng |
|---|---|
| `--net` | Thêm bắt network: mitmproxy trên Mac; PCAPdroid, api key, quyền VPN trên máy (tự động hết) |
| `--with-ca` | Đẩy CA mitmproxy lên máy để giải mã HTTPS (cài CA là việc làm tay, xem dưới) |
| `--serial <id>` | Chỉ một máy |
| `--yes` | Không hỏi (tự thêm quyền `mcp__mobile-lite`) |
| `--no-register` | Bỏ qua bước Claude Code |

Đo trên ZTE Blade A54 đã gỡ sạch: `scripts/setup.sh --net` mất 2 phút 36 giây (gồm tải APK), không phải chạm vào máy.
Không `--net`, máy đã có UiAutomator2: 27 giây (gồm build).

Lệnh lẻ vẫn còn: `host` (chỉ Mac), `device [serial]` (một máy, nhận `--net`/`--with-ca`), `register`, `verify [serial]`, `clean [serial]`.

Sau đó mở phiên Claude Code mới và giao việc, ví dụ: *"Dùng android-tester mở Cài đặt và cho biết Dark mode đang bật hay tắt"*.

## Subagent `android-tester`

Setup cài subagent [`.claude/agents/android-tester.md`](../.claude/agents/android-tester.md): chỉ được dùng tool `mobile-lite`,
chạy Sonnet, có sẵn quy tắc an toàn (không gửi tin, gọi điện, thanh toán, đổi cài đặt nếu nhiệm vụ không yêu cầu đúng việc đó)
và báo cáo dạng PASS/FAIL kèm dòng màn hình làm bằng chứng.

- Trong Claude Code: *"Dùng android-tester kiểm tra luồng đăng nhập của app com.example: …"* — agent chính giao việc, chỉ nhận lại báo cáo.
- Chạy riêng, rẻ nhất (cho CI hoặc script): `scripts/run-test.sh "Mở Cài đặt, vào Display: Dark mode đang On hay Off?"`.
  Dùng prompt của agent làm toàn bộ system prompt, không tool có sẵn, chỉ MCP mobile-lite; in báo cáo và một dòng
  `[lượt, giây, $]`, exit 1 nếu không PASS. `MODEL=haiku` để rẻ hơn, `STREAM=run.jsonl` để giữ transcript.
  Đo trên ZTE Blade A54, smoke test 3 phần (UI, gõ tiếng Việt, network Zalo): 7 lượt, 38 s, $0,037
  (`claude -p --agent android-tester`: 9 lượt, $0,042).
- Gọi qua phiên Claude Code chính không bị hỏi quyền từng thao tác khi `~/.claude/settings.json` có
  `"permissions": {"allow": ["mcp__mobile-lite"]}` (setup hỏi rồi thêm). Subagent rẻ ($0,019 cho một kiểm tra) nhưng phiên chính (Opus) vẫn
  tốn phần lớn ($0,17), vì nó mang system prompt đầy đủ.

## Việc phải làm tay

| Khi nào | Làm gì |
|---|---|
| Lần đầu cắm máy | Bật Gỡ lỗi USB và bấm **Cho phép** (xem trên). |
| Chỉ khi cần **giải mã HTTPS** (`--with-ca`) | Script đã đẩy `Download/mitmproxy-ca-cert.crt` và mở màn Bảo mật. Vào *Mã hoá & thông tin xác thực > Cài chứng chỉ > Chứng chỉ CA > Vẫn cài*, nhập PIN, chọn file. Kiểm tra ở *Thông tin xác thực đáng tin cậy > Người dùng* có `mitmproxy`. Đường dẫn menu khác nhau tuỳ hãng. |
| Nếu `--net` báo không tự tạo được api key | PCAPdroid > Settings > Control permissions > ⋮ > Generate API key, rồi `echo 'PCAPDROID_API_KEY_<serial>=<key>' >> ~/.config/mobile-mcp-lite/keys.env` (không cần `register` lại). Bước tự động dựa vào chữ tiếng Anh, máy để ngôn ngữ khác có thể không khớp. |

Android không cho cài CA người dùng hay cấp quyền VPN qua adb khi không root, nên các bước này không thể tự động hết.

## Script làm gì

| Bước | Chi tiết |
|---|---|
| Mac | `brew install` adb, Node nếu thiếu (mitmproxy khi `--net`), `npm ci --ignore-scripts`, build `lib/` khi mã đổi. OCR dùng `swiftc` nếu có (Xcode command line tools). |
| UiAutomator2 server 10.6.6 | 2 APK chính thức của Appium: đọc màn hình ~0,2 s thay vì ~2,7 s, gõ được tiếng Việt. |
| PCAPdroid 2.0.2 (F-Droid), `--net` | Bắt traffic theo từng app qua VPN cục bộ, không root. Cấp quyền thông báo, tạo api key, chấp nhận quyền VPN lần đầu. |
| CA mitmproxy, `--with-ca` | Đẩy file chứng chỉ lên máy; cài là việc làm tay. |
| Claude Code | `claude mcp add -s user mobile-lite -- node lib/index.js`, symlink subagent, quyền `mcp__mobile-lite`. Server tự đọc api key từ `~/.config/mobile-mcp-lite/keys.env`, nên thêm máy mới không cần đăng ký lại. |

**An toàn chuỗi cung ứng:** mọi APK được ghim SHA-256 trong `scripts/setup.sh`; file tải về khác hash thì script dừng, không cài.
- UiAutomator2: APK của release [v10.6.6](https://github.com/appium/appium-uiautomator2-server/releases/tag/v10.6.6) trùng từng byte với APK trong gói npm `appium-uiautomator2-server@10.6.6` (có SLSA provenance).
- PCAPdroid: bản F-Droid tự build từ mã nguồn; `classes*.dex` trùng với bản tác giả ký trên GitHub (thư viện `.so` khác do build lại).

**Nơi lưu bí mật:** api key PCAPdroid chỉ ở `~/.config/mobile-mcp-lite/keys.env` (quyền 600; đổi đường dẫn bằng `MOBILEMCP_KEYS_FILE`). Khoá riêng của CA mitmproxy ở `~/.mitmproxy/`: ai có file này đọc được HTTPS của các app tin CA người dùng trên máy đã cài CA, nên chỉ cài CA trên máy test.

## Cho app của bạn: bật giải mã HTTPS ở bản debug

Từ Android 7 app không tin CA người dùng trừ khi khai báo. Thêm vào **bản debug** (không đưa vào release):

`app/src/debug/res/xml/network_security_config.xml`
```xml
<network-security-config>
  <debug-overrides>
    <trust-anchors>
      <certificates src="user" />
    </trust-anchors>
  </debug-overrides>
</network-security-config>
```

`app/src/debug/AndroidManifest.xml`
```xml
<application android:networkSecurityConfig="@xml/network_security_config" />
```

Sau đó dùng `net start {decrypt:"api\\.myapp\\.com"}` để xem header và body của request tới host đó.

## Hai cách bắt network

| | `net start` | `net start {app:"<package>"}` |
|---|---|---|
| Công cụ | mitmproxy qua `adb reverse` + proxy hệ thống | PCAPdroid (VPN theo app) |
| Thấy app nào | App dùng proxy hệ thống (OkHttp, HttpURLConnection) | Mọi kết nối của app đó (cả Zalo, Flutter, socket riêng) |
| Nội dung | Header + body nếu `decrypt` khớp và app tin CA | Chỉ domain, cổng, dung lượng |

Luôn `net stop` khi xong. Server cũng tự gỡ proxy khi thoát.

## Xử lý sự cố

| Triệu chứng | Cách xử lý |
|---|---|
| Script chờ mãi ở bước Điện thoại | Làm theo dòng *LÀM TAY* nó in; `adb devices` phải hiện `device`. Chờ lâu hơn: `DEVICE_WAIT_SECONDS=900`. |
| Nhiều máy cắm cùng lúc | Script cài cho tất cả; chỉ một máy thì `--serial <id>`. |
| `adb devices` hiện `unauthorized` | Mở khoá máy, bấm Cho phép trên hộp thoại USB debugging; hoặc *Thu hồi quyền gỡ lỗi USB* rồi cắm lại. |
| Điện thoại mất internet sau khi dùng `net` | Proxy bị treo do server bị kill cứng. Lần tới server chạm vào máy sẽ tự gỡ; gỡ ngay: `adb shell settings put global http_proxy :0` |
| Biểu tượng VPN còn hiện | Mở PCAPdroid bấm Stop, hoặc `scripts/setup.sh verify` để xem trạng thái. |
| Đọc màn hình chậm ~2,7 s | UiAutomator2 server chưa cài hoặc đang tạm tắt 60 s sau lỗi; chạy lại `scripts/setup.sh`. |
| Thiếu một view không có nhãn (vd. RecyclerView trống) | Server bật `ignoreUnimportantViews` cho nhanh; đặt `MOBILEMCP_ALL_VIEWS=1` để lấy đủ. |
| `net start {app}` vẫn hỏi Allow trên máy | Thiếu api key cho serial đó trong `keys.env`: chạy lại `scripts/setup.sh --net --serial <serial>`. |
| Đọc màn hình lỗi liên tục trên một màn | Server tự chuyển sang `uiautomator dump` trong 60 s. Bản UiAutomator2 7.x lỗi `mSealed` với lớp phủ "đã sao chép" của Android 13; 10.6.6 đã hết. |

## Gỡ bỏ

```bash
scripts/setup.sh clean [serial]   # gỡ UiAutomator2, PCAPdroid, proxy, file CA đã đẩy, xoá key của máy
```
CA người dùng phải gỡ tay: *Thông tin xác thực đáng tin cậy > Người dùng > mitmproxy > Xoá*.
Gỡ MCP: `claude mcp remove mobile-lite -s user`.

## Nâng phiên bản APK

1. Tải APK mới từ nguồn chính thức (GitHub release của Appium; F-Droid cho PCAPdroid).
2. Với PCAPdroid, so `classes*.dex` của bản F-Droid với bản tác giả trên GitHub (`unzip` rồi `shasum -a 256`).
3. Cập nhật version, tên file và SHA-256 ở đầu `scripts/setup.sh`, chạy `clean` rồi `scripts/setup.sh --net` trên một máy test.
