# Cài berth-mobile (điều khiển Android và iPhone qua USB)

## Cách 1: trong Shipyard (khuyên dùng)

Settings → Plugins → **Install from URL** →
`https://github.com/tinhthanh/vetgo-releases/releases/download/mobile-v<phiên bản>/mobile-<phiên bản>.tar.gz`
(để trống ô SHA-256) → **Allow**. Setup tự chạy: tải adb nếu cần, cài UiAutomator2 lên điện thoại đang cắm.
Trong Settings → Plugins → Mobile có hai switch:

- **Tự cài UiAutomator2 / WebDriverAgent**: bật sẵn.
- **Cho agent bắt lưu lượng mạng (net)**: tắt sẵn, bật sẽ hỏi lại. Chỉ bật trên máy test.

## Cách 2: không có Shipyard

```bash
git clone https://github.com/tinhthanh/mobile-mcp.git && mobile-mcp/scripts/setup.sh --yes
```

Cần Rust (https://rustup.rs) và Xcode Command Line Tools. Script build berth-mobile, chạy `berth-mobile setup`,
đăng ký MCP `berth-mobile` và subagent `android-tester` trong Claude Code.

## Android

Bật Tuỳ chọn nhà phát triển (bấm 7 lần vào Số hiệu bản dựng) và Gỡ lỗi USB, cắm cáp dữ liệu, bấm **Cho phép**
(nên tick *Luôn cho phép*). `devices` báo `unauthorized` khi chưa cho phép.

## iPhone (làm một lần cho mỗi Mac)

1. iPhone: cắm cáp, bấm **Tin cậy**, bật **Cài đặt → Quyền riêng tư & Bảo mật → Chế độ nhà phát triển**.
2. Mac: cần bộ ký của team Apple Developer trong `~/Library/Application Support/berth-mobile/signing/`
   (keychain + profile có UDID của iPhone). Có bản sao lưu thì khôi phục:
   `berth-mobile/scripts/signing-backup.sh restore berth-mobile/secrets/berth-mobile-signing.tar.enc`
3. iPhone mới: thêm UDID vào profile "berth-mobile WDA" trên developer.apple.com, tải profile về thay
   `signing/wda.mobileprovision`.

Lần dùng đầu plugin tự tải go-ios, ký và cài WebDriverAgent, mount developer image (10–60 giây). Giữ iPhone mở
khoá khi agent làm việc: khi khoá, agent sẽ dừng và nhờ người mở.

## Xem và điều khiển điện thoại Android trên Mac

`scripts/setup.sh mirror` (scrcpy qua USB, không cần app trên điện thoại); `scripts/setup.sh mirror install` để
tự mở mỗi khi cắm.

## Xử lý sự cố

- **Không thấy máy Android dù đã cắm**: rút cắm lại, tắt/bật Gỡ lỗi USB (adbd trên máy có thể bị treo).
- **iPhone có trong ioreg nhưng `devices` không thấy**: mở khoá iPhone, bấm Tin cậy nếu được hỏi, rút cắm lại.
- **"WebDriverAgent did not start"**: mở khoá iPhone; xem `~/Library/Application Support/berth-mobile/run/<udid>/wda.log`.
- **Mất mạng trên điện thoại sau khi bắt network**: lần chạy kế tiếp tự gỡ proxy; gỡ tay:
  `adb shell settings put global http_proxy :0`.
- Log setup: `~/Library/Application Support/berth-mobile/setup.log`.
