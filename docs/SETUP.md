# Setup mobile-mcp-lite cho một máy Android mới

Hướng dẫn cài để Claude Code (hoặc agent khác qua MCP) điều khiển và bắt network của một điện thoại Android qua USB.
Đã kiểm chứng trên ZTE Blade A54, Android 13, không root.

## Nhanh: 4 lệnh

```bash
git clone -b lite https://github.com/tinhthanh/mobile-mcp.git && cd mobile-mcp
scripts/setup.sh host            # một lần trên Mac: adb, mitmproxy, build
scripts/setup.sh device          # mỗi điện thoại (thêm <serial> nếu cắm nhiều máy)
scripts/setup.sh register        # một lần cho mỗi Mac: đăng ký MCP "mobile-lite" vào Claude Code
scripts/setup.sh verify          # kiểm tra lại
```

Sau đó mở phiên Claude Code mới và giao việc, ví dụ: *"Mở Zalo, bắt traffic 15 giây và cho tôi biết nó gọi tới domain nào"*.

## Việc phải làm tay

| Khi nào | Làm gì |
|---|---|
| Trước `device` | Bật **Tùy chọn nhà phát triển** (bấm 7 lần vào *Số hiệu bản dựng*), bật **Gỡ lỗi USB**, cắm cáp, bấm **Cho phép** trên hộp thoại *Allow USB debugging?* (tick *Luôn cho phép*). `adb devices` phải hiện `device`, không phải `unauthorized`. |
| Chỉ khi cần **giải mã HTTPS** | Script đã đẩy `Download/mitmproxy-ca-cert.crt` và mở màn Bảo mật. Vào *Mã hoá & thông tin xác thực > Cài chứng chỉ > Chứng chỉ CA > Vẫn cài*, nhập PIN, chọn file. Kiểm tra ở *Thông tin xác thực đáng tin cậy > Người dùng* có `mitmproxy`. Đường dẫn menu khác nhau tuỳ hãng. |
| Nếu `device` báo không tự tạo được api key | PCAPdroid > Settings > Control permissions > ⋮ > Generate API key, rồi `echo 'PCAPDROID_API_KEY_<serial>=<key>' >> ~/.config/mobile-mcp-lite/keys.env` (không cần `register` lại). Bước tự động dựa vào chữ tiếng Anh, máy để ngôn ngữ khác có thể không khớp. |

Android không cho cài CA người dùng hay cấp quyền VPN qua adb khi không root, nên các bước này không thể tự động hết.

## Script làm gì

| Bước | Chi tiết |
|---|---|
| `host` | `brew install --cask android-platform-tools mitmproxy`, `npm ci --ignore-scripts`, build `lib/`. OCR dùng `swiftc` nếu có (Xcode command line tools). |
| UiAutomator2 server 7.0.22 | 2 APK chính thức của Appium: đọc màn hình ~0,4 s thay vì ~2,7 s, gõ được tiếng Việt. |
| PCAPdroid 2.0.2 (F-Droid) | Bắt traffic theo từng app qua VPN cục bộ, không root. Cấp quyền thông báo, tạo api key, chấp nhận quyền VPN lần đầu. |
| CA mitmproxy | Đẩy file chứng chỉ lên máy; cài là việc làm tay. |
| `register` | `claude mcp add -s user mobile-lite -- node lib/index.js`. Server tự đọc api key từ `~/.config/mobile-mcp-lite/keys.env`, nên thêm máy mới không cần đăng ký lại. |

**An toàn chuỗi cung ứng:** mọi APK được ghim SHA-256 trong `scripts/setup.sh`; file tải về khác hash thì script dừng, không cài.
- UiAutomator2: trùng từng byte với release [v7.0.22](https://github.com/appium/appium-uiautomator2-server/releases/tag/v7.0.22).
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
| `adb devices` hiện `unauthorized` | Mở khoá máy, bấm Cho phép trên hộp thoại USB debugging; hoặc *Thu hồi quyền gỡ lỗi USB* rồi cắm lại. |
| Điện thoại mất internet sau khi dùng `net` | Proxy bị treo do server bị kill cứng. Lần tới server chạm vào máy sẽ tự gỡ; gỡ ngay: `adb shell settings put global http_proxy :0` |
| Biểu tượng VPN còn hiện | Mở PCAPdroid bấm Stop, hoặc `scripts/setup.sh verify` để xem trạng thái. |
| Đọc màn hình chậm ~2,7 s | UiAutomator2 server chưa cài hoặc đang tạm tắt 60 s sau lỗi; chạy lại `scripts/setup.sh device`. |
| `net start {app}` vẫn hỏi Allow trên máy | Thiếu api key cho serial đó trong `keys.env`: chạy lại `scripts/setup.sh device <serial>`. |
| Màn hình Android 13 có lớp phủ "đã sao chép" không đọc được | Lỗi `mSealed` của UiAutomator2; server tự chuyển sang `uiautomator dump` trong 60 s. |

## Gỡ bỏ

```bash
scripts/setup.sh clean [serial]   # gỡ UiAutomator2, PCAPdroid, proxy, file CA đã đẩy, xoá key của máy
```
CA người dùng phải gỡ tay: *Thông tin xác thực đáng tin cậy > Người dùng > mitmproxy > Xoá*.
Gỡ MCP: `claude mcp remove mobile-lite -s user`.

## Nâng phiên bản APK

1. Tải APK mới từ nguồn chính thức (GitHub release của Appium; F-Droid cho PCAPdroid).
2. Với PCAPdroid, so `classes*.dex` của bản F-Droid với bản tác giả trên GitHub (`unzip` rồi `shasum -a 256`).
3. Cập nhật version, tên file và SHA-256 ở đầu `scripts/setup.sh`, chạy `clean` rồi `device` trên một máy test.
