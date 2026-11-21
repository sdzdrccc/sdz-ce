//! sdz-ce：轻量护眼工具（DWM 调色层方案，不修改系统 gamma 表）
//!
//! 界面（还原"护眼宝"风格）：左侧栏（护眼/健康/关于）+ 环形百分比 + 强度滑条
//!   + 5 个颜色圆点 + 夜间助眠开关 + 右上角 ☰/✕
//! 护眼核心：全屏点击穿透分层窗叠加暖色（SetLayeredWindowAttributes），
//!   进程消失即恢复，零残留；覆盖全部虚拟屏幕。
//! 配套：20-20-20 休息提醒、系统托盘、全局热键（F9 护眼开关 / F8 休息 / F5 主窗口）

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::Debug::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------- 布局常量 ----------------
const WIN_W: i32 = 480;
const WIN_H: i32 = 285;

// 侧栏（参照 90px 宽 ×0.75）；三个导航块 (y0,y1)
const SIDE_W: i32 = 67;
const NAV_EYE: (i32, i32) = (45, 106);
const NAV_HEALTH: (i32, i32) = (106, 172);
const NAV_ABOUT: (i32, i32) = (172, 233);

// 右上角 ☰ / ✕
const MENU_X0: i32 = 431;
const MENU_X1: i32 = 445;
const CLOSE_X0: i32 = 452;
const CLOSE_X1: i32 = 466;
const TOP_Y0: i32 = 8;
const TOP_Y1: i32 = 30;

// 内容中心 x（眼睛环中心）
const CX: i32 = 263;

// 眼睛徽章（emblem.bmp 188×188，显示 63×63，环中心对齐 CX）
const EMB_SZ: i32 = 63;
const EMB_X: i32 = CX - EMB_SZ / 2;
const EMB_Y: i32 = 24;
const STATUS_Y: i32 = 104;

// 滑条（青→绿→黄渐变）
const SL_X0: i32 = 117;
const SL_X1: i32 = 316;
const SL_Y: i32 = 129;

// 标签行 + 夜间开关
const LABEL_Y: i32 = 164;
const TOG_LBL_X: i32 = 352;
const TOG_X: i32 = 456;
const TOG_Y: i32 = 168;
const TOG_W: i32 = 25;
const TOG_H: i32 = 13;

// 5 色圆点
const CIRCLE_Y: i32 = 207;
const CIRCLE_R: i32 = 7;
const CIRCLE_XS: [i32; 5] = [150, 213, 267, 322, 381];
const CAPTION_Y: i32 = 234;

// 健康页
const HP_ROW1_Y: i32 = 60;
const HP_ROW2_Y: i32 = 110;
const HP_ROW3_Y: i32 = 160;
const HP_VAL_X: i32 = 330;

// 健康页数值循环
const REST_VALUES: [u8; 5] = [5, 10, 20, 30, 60];
const FOCUS_VALUES: [u8; 5] = [10, 20, 30, 45, 60];

// ---------------- 颜色（护眼宝青色系，参照原 exe 实测取色） ----------------
// COLORREF = 0x00BBGGRR
const TEAL: COLORREF = COLORREF(0x00ABB64F); // #4FB6AB 主青（侧栏/圆环/开关）
const TEAL_DARK: COLORREF = COLORREF(0x00555B27); // #275B55 侧栏选中深块
const MAIN_BG: COLORREF = COLORREF(0x00EBEBEB); // #EBEBEB 浅灰内容底
const WHITE: COLORREF = COLORREF(0x00FFFFFF);
const TEXT_DARK: COLORREF = COLORREF(0x00343434); // #343434 正文
const TEXT_GRAY: COLORREF = COLORREF(0x009A9A9A); // #9A9A9A 副文字
const LINE_GRAY: COLORREF = COLORREF(0x00C8C8C8); // 未填充轨道/线
const SL_YEL: COLORREF = COLORREF(0x004AE8E0); // #E0E84A 滑条末端黄
const CIRCLE_DARK: COLORREF = COLORREF(0x00141414);

// 色温圆点（0 淡黄 / 1 黄 / 2 绿 / 3 橙红 / 4 黑=关闭）—— 真正叠加到屏幕的护眼色，保持不变
const CIRCLE_COLORS: [COLORREF; 5] = [
    COLORREF(0x00C0E8F7),
    COLORREF(0x003CE0F0),
    COLORREF(0x0050AF4C),
    COLORREF(0x002F56F0),
    CIRCLE_DARK,
];
const CIRCLE_NAMES: [&str; 5] = ["淡黄", "黄", "绿", "橙红", "关闭"];

// ---------------- 类名 / 消息常量 ----------------
const CLASS_HIDDEN: &str = "sdz_ce_hidden";
const CLASS_TINT: &str = "sdz_ce_tint";
const CLASS_HUD: &str = "sdz_ce_hud";
const CLASS_MAIN: &str = "sdz_ce_main";

const TRAY_ID: u32 = 0xC3E;
const TRAY_CALLBACK: u32 = WM_USER + 1;
const TIMER_IDLE: u32 = 1;
const TIMER_HUD: u32 = 2;

const HK_TOGGLE: i32 = 1; // F9
const HK_BREAK: i32 = 2;  // F8
const HK_SHOW: i32 = 3;   // F5

const EVENT_LEFTBUTTONUP: u32 = 0x0202; // WM_LBUTTONUP
const EVENT_RIGHTBUTTONUP: u32 = 0x0205; // WM_RBUTTONUP

const HUD_TEXT: u32 = 9001;

// 主窗命中区
const HIT_NONE: u32 = 0;
const HIT_PAGE_EYE: u32 = 1;
const HIT_PAGE_HEALTH: u32 = 2;
const HIT_PAGE_ABOUT: u32 = 3;
const HIT_SLIDER: u32 = 4;
const HIT_CIRC_BASE: u32 = 10; // +0..4
const HIT_TOG_NIGHT: u32 = 20;
const HIT_MENU: u32 = 21;
const HIT_CLOSE: u32 = 22;
const HIT_TOG_BREAK: u32 = 23;
const HIT_REST: u32 = 24;
const HIT_FOCUS: u32 = 25;

// 主窗 ☰ 菜单
const CM_AUTOSTART: u32 = 101;
const CM_SYSNIGHT: u32 = 102;
const CM_EXIT: u32 = 103;

// ---------------- 配置 ----------------
#[derive(Clone, Copy)]
struct Cfg {
    intensity: u8, // 0..100 护眼强度（分层窗 alpha）
    temp: u8,      // 0..4（4 = 关闭）
    rest_min: u8,
    focus_min: u8,
    auto_break: bool,
    auto_time: bool,
    autostart: bool,
    auto_on_hm: u16,
    auto_off_hm: u16,
}

impl Cfg {
    fn new_default() -> Cfg {
        Cfg {
            intensity: 40,
            temp: 1,
            rest_min: 20,
            focus_min: 20,
            auto_break: true,
            auto_time: false,
            autostart: false,
            auto_on_hm: 21 * 60,
            auto_off_hm: 7 * 60,
        }
    }
}

fn cfg_path() -> std::path::PathBuf {
    let a = std::env::var("APPDATA").map(std::path::PathBuf::from).unwrap_or_default();
    let d = a.join("sdz-ce");
    let _ = std::fs::create_dir_all(&d);
    d.join("config.ini")
}

fn load_cfg() -> Cfg {
    let mut c = Cfg::new_default();
    if let Ok(t) = std::fs::read_to_string(cfg_path()) {
        for line in t.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                match k.trim() {
                    "intensity" => c.intensity = v.parse().unwrap_or(40).min(100),
                    "temp" => c.temp = v.parse().unwrap_or(1).min(4),
                    "rest_min" => c.rest_min = v.parse().unwrap_or(20).clamp(1, 60),
                    "focus_min" => c.focus_min = v.parse().unwrap_or(20).clamp(5, 120),
                    "auto_break" => c.auto_break = v == "1",
                    "auto_time" => c.auto_time = v == "1",
                    "autostart" => c.autostart = v == "1",
                    "auto_on_hm" => c.auto_on_hm = v.parse().unwrap_or(21 * 60),
                    "auto_off_hm" => c.auto_off_hm = v.parse().unwrap_or(7 * 60),
                    _ => {}
                }
            }
        }
    }
    c
}

fn save_cfg(c: &Cfg) {
    let txt = format!(
        "[sdz-ce]\nintensity={}\ntemp={}\nrest_min={}\nfocus_min={}\n\
         auto_break={}\nauto_time={}\nautostart={}\nauto_on_hm={}\nauto_off_hm={}\n",
        c.intensity, c.temp, c.rest_min, c.focus_min,
        c.auto_break as u8, c.auto_time as u8, c.autostart as u8,
        c.auto_on_hm, c.auto_off_hm
    );
    let _ = std::fs::write(cfg_path(), txt);
}

// ---------------- 全局 ----------------
struct App {
    hidden: HWND,
    tint: HWND,
    hud: HWND,
    main_w: HWND,
    emblem: HBITMAP,
    cfg: Cfg,
    page: u8,
    in_break: bool,
    hud_left: u32,
    mouse_down: bool,
    drag: Option<(i32, i32)>, // 无边框窗口拖动（按住顶部空白区）
}

static mut APP: *mut App = ptr::null_mut();
static mut MUTEX: HANDLE = HANDLE(ptr::null_mut());

// ---------------- 开机自启 ----------------
fn apply_autostart(enable: bool) {
    let mut exe: Vec<u16> = Vec::new();
    if let Ok(p) = std::env::current_exe() {
        if p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_lowercase().ends_with("sdz-ce.exe"))
            .unwrap_or(false)
        {
            let s = format!("\"{}\"", p.display());
            exe = s.encode_utf16().chain(std::iter::once(0)).collect();
        }
    }
    if exe.is_empty() {
        return;
    }
    let sub = "Software\\Microsoft\\Windows\\CurrentVersion\\Run"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let name = "sdz-ce".encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
    unsafe {
        let mut key = HKEY::default();
        let err = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if err.0 != 0 {
            return;
        }
        if enable {
            let bytes = std::slice::from_raw_parts(
                exe.as_ptr() as *const u8,
                exe.len() * std::mem::size_of::<u16>(),
            );
            let _ = RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes));
        } else {
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        }
    }
}

// ---------------- 护眼状态 ----------------
fn in_auto_window(c: &Cfg) -> bool {
    unsafe {
        let lt = GetLocalTime();
        let now = (lt.wHour as u16) * 60 + lt.wMinute as u16;
        let on = c.auto_on_hm;
        let off = c.auto_off_hm;
        if on <= off {
            now >= on && now < off
        } else {
            now >= on || now < off
        }
    }
}

fn is_tinting(a: &App) -> bool {
    let c = &a.cfg;
    if c.temp >= 4 || c.intensity == 0 {
        return false;
    }
    if c.auto_time {
        in_auto_window(c)
    } else {
        true
    }
}

fn status_text(a: &App) -> &'static str {
    let c = &a.cfg;
    if c.temp >= 4 || c.intensity == 0 {
        "护眼已关闭"
    } else if c.auto_time && in_auto_window(c) {
        "夜间助眠生效中"
    } else if c.auto_time {
        "夜间助眠待生效"
    } else {
        "手动护眼中"
    }
}

// ---------------- 托盘 ----------------
fn tray_tip(a: &App) -> String {
    if a.in_break {
        format!("sdz-ce：休息中 {}:{:02}", a.hud_left / 60, a.hud_left % 60)
    } else if is_tinting(a) {
        format!(
            "sdz-ce：护眼中 · {}% · {}",
            a.cfg.intensity,
            CIRCLE_NAMES[a.cfg.temp as usize]
        )
    } else {
        "sdz-ce：护眼已关闭".into()
    }
}

fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

fn tray_icon() -> HICON {
    unsafe {
        LoadImageW(None, IDC_ARROW, IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_default()
    }
}

fn tray_data(a: &App) -> NOTIFYICONDATAW {
    let tip = tray_tip(a);
    let t: Vec<u16> = tip.encode_utf16().collect();
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK,
        hIcon: tray_icon(),
        ..Default::default()
    };
    d.hWnd = a.hidden;
    let n = t.len().min(127);
    d.szTip[..n].copy_from_slice(&t[..n]);
    d
}

// ---------------- 调色层（护眼核心） ----------------
unsafe extern "system" fn wnd_tint(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // 分层窗的像素内容决定最终叠色：必须在 WM_PAINT 里把整个客户区填成目标色
    if m == WM_PAINT {
        let a = &*APP;
        let color = if !is_tinting(a) {
            COLORREF(0)
        } else {
            CIRCLE_COLORS[a.cfg.temp as usize]
        };
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(h, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(h, &mut rc);
        let _ = FillRect(hdc, &rc, CreateSolidBrush(color));
        let _ = EndPaint(h, &ps);
        LRESULT(0)
    } else {
        DefWindowProcW(h, m, w, l)
    }
}

fn apply_tint(a: &App) {
    let active = is_tinting(a);
    let alpha = if active { a.cfg.intensity } else { 0 };
    unsafe {
        // 颜色画在窗口像素里（wnd_tint 的 WM_PAINT），这里只调透明度；
        // 先失效客户区触发重绘，新颜色才能生效
        let _ = InvalidateRect(a.tint, None, TRUE);
        SetLayeredWindowAttributes(a.tint, COLORREF(0), alpha, LWA_ALPHA).ok();
    }
}

fn virtual_screen_rect() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

fn create_tint(a: &mut App) {
    let (vx, vy, vw, vh) = virtual_screen_rect();
    unsafe {
        let cls = CLASS_TINT.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_tint),
            hInstance: hinst(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        a.tint = CreateWindowExW(
            WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR(cls.as_ptr()),
            None,
            WS_POPUP,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            hinst(),
            None,
        )
        .unwrap_or_default();
        apply_tint(a);
        let _ = ShowWindow(a.tint, SW_SHOWNOACTIVATE);
    }
}

// ---------------- 休息提示 ----------------
fn menu_id_h(id: u32) -> HMENU {
    HMENU(id as usize as *mut core::ffi::c_void)
}

fn set_hud_text(h: HWND, text: &str) {
    unsafe {
        let mut u16: Vec<u16> = text.encode_utf16().collect();
        u16.push(0);
        if let Ok(st) = GetDlgItem(h, HUD_TEXT as i32) {
            let _ = SendMessageW(st, WM_SETTEXT, WPARAM(u16.len()), LPARAM(u16.as_ptr() as isize));
        }
    }
}

fn create_hud(a: &mut App) {
    let (vx, vy, vw, vh) = virtual_screen_rect();
    unsafe {
        if !a.hud.is_invalid() {
            return;
        }
        let cls = CLASS_HUD.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_null),
            hInstance: hinst(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let w = 420i32;
        let h = 150i32;
        a.hud = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(cls.as_ptr()),
            None,
            WS_POPUP,
            vx + vw / 2 - w / 2,
            vy + vh / 3,
            w,
            h,
            None,
            None,
            hinst(),
            None,
        )
        .unwrap_or_default();
        if a.hud.is_invalid() {
            return;
        }
        let st = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR("STATIC".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>()[..].as_ptr()),
            None,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | 1u32), // SS_CENTER
            0,
            0,
            w,
            h,
            a.hud,
            menu_id_h(HUD_TEXT),
            hinst(),
            None,
        )
        .unwrap_or_default();
        let face = "Segoe UI".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let f = CreateFontW(
            -42, 0, 0, 0, 700, 0, 0, 0,
            DEFAULT_CHARSET.0.into(),
            OUT_TT_PRECIS.0.into(),
            CLIP_DEFAULT_PRECIS.0.into(),
            CLEARTYPE_QUALITY.0.into(),
            (DEFAULT_PITCH.0 | FIXED_PITCH.0).into(),
            PCWSTR(face.as_ptr()),
        );
        if !f.is_invalid() {
            let _ = SendMessageW(st, WM_SETFONT, WPARAM(f.0 as usize), LPARAM(1));
        }
    }
}

fn show_hud(a: &App) {
    set_hud_text(
        a.hud,
        &format!(
            "休息中 {}:{:02}　·　看 20 英尺（6 米）远处",
            a.hud_left / 60,
            a.hud_left % 60
        ),
    );
}

fn start_break(a: &mut App, secs: u32) {
    unsafe {
        if a.in_break {
            return;
        }
        a.in_break = true;
        a.hud_left = secs.max(5);
        create_hud(a);
        if a.hud.is_invalid() {
            a.in_break = false;
            a.hud_left = 0;
            return;
        }
        show_hud(a);
        let _ = ShowWindow(a.hud, SW_SHOWNOACTIVATE);
        SetTimer(a.hidden, TIMER_HUD as usize, 1000, None);
        MessageBeep(MB_ICONASTERISK).ok();
        let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
    }
}

fn hud_tick(a: &mut App) {
    a.hud_left = a.hud_left.saturating_sub(1);
    unsafe {
        if a.hud_left == 0 {
            a.in_break = false;
            let _ = ShowWindow(a.hud, SW_HIDE);
            KillTimer(a.hidden, TIMER_HUD as usize).ok();
            MessageBeep(MB_ICONASTERISK).ok();
            let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
            return;
        }
        show_hud(a);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
    }
}

// ---------------- 空闲 + 自动时段 tick ----------------
fn idle_millis() -> u32 {
    unsafe {
        let mut lii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        let _ = GetLastInputInfo(&mut lii);
        GetTickCount().wrapping_sub(lii.dwTime)
    }
}

fn refresh_main(a: &App) {
    unsafe {
        let h = a.main_w;
        if IsWindow(h).0 != 0 && IsWindowVisible(h).0 != 0 {
            let _ = InvalidateRect(h, None, TRUE);
        }
    }
}

fn tick_idle(a: &mut App) {
    if !a.in_break && a.cfg.auto_break {
        let focus_ms = (a.cfg.focus_min as u32) * 60_000;
        if idle_millis() >= focus_ms {
            start_break(a, (a.cfg.rest_min as u32) * 60);
            return;
        }
    }
    if a.cfg.auto_time {
        apply_tint(a);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
        }
        refresh_main(a);
    }
}

// ---------------- GDI 绘制辅助 ----------------
fn font_seg_ui(size: i32, weight: i32) -> HFONT {
    unsafe {
        let face = "Segoe UI".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        CreateFontW(
            -size, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET.0.into(),
            OUT_TT_PRECIS.0.into(),
            CLIP_DEFAULT_PRECIS.0.into(),
            CLEARTYPE_QUALITY.0.into(),
            (DEFAULT_PITCH.0 | FIXED_PITCH.0).into(),
            PCWSTR(face.as_ptr()),
        )
    }
}

fn draw_text(hdc: HDC, f: HFONT, x: i32, y: i32, s: &str, color: COLORREF) {
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, color);
        let old = SelectObject(hdc, f);
        let mut u16: Vec<u16> = s.encode_utf16().collect();
        u16.push(0);
        let _ = TextOutW(hdc, x, y, &u16);
        let _ = SelectObject(hdc, old);
    }
}

fn text_width(hdc: HDC, f: HFONT, s: &str) -> i32 {
    unsafe {
        let old = SelectObject(hdc, f);
        let mut u16: Vec<u16> = s.encode_utf16().collect();
        u16.push(0);
        let mut sz = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &u16, &mut sz);
        let _ = SelectObject(hdc, old);
        sz.cx
    }
}

fn text_center(hdc: HDC, f: HFONT, cx: i32, y: i32, s: &str, color: COLORREF) {
    let w = text_width(hdc, f, s);
    draw_text(hdc, f, cx - w / 2, y, s, color);
}

fn pen_w(hdc: HDC, width: i32, color: COLORREF) -> HGDIOBJ {
    unsafe {
        let p = CreatePen(PEN_STYLE(PS_SOLID.0), width, color);
        SelectObject(hdc, p)
    }
}

fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let m = |ca: u8, cb: u8| ((ca as f32 + (cb as f32 - ca as f32) * t).clamp(0.0, 255.0)) as u32;
    let ar = (a & 0xFF) as u8;
    let ag = ((a >> 8) & 0xFF) as u8;
    let ab = ((a >> 16) & 0xFF) as u8;
    let br = (b & 0xFF) as u8;
    let bg = ((b >> 8) & 0xFF) as u8;
    let bb = ((b >> 16) & 0xFF) as u8;
    m(ar, br) | (m(ag, bg) << 8) | (m(ab, bb) << 16)
}

fn draw_arc_poly(hdc: HDC, cx: i32, cy: i32, r: i32, start_deg: f32, sweep_deg: f32) {
    // 手动折线绘制圆弧（windows 0.58 无 DrawArc）
    unsafe {
        let steps = 64usize;
        let start = (start_deg) * std::f32::consts::PI / 180.0;
        let d = sweep_deg * std::f32::consts::PI / 180.0;
        for i in 0..=steps {
            let a = start + d * (i as f32 / steps as f32);
            let x = cx + ((r as f32) * a.cos()).round() as i32;
            let y = cy + ((r as f32) * a.sin()).round() as i32;
            if i == 0 {
                let _ = MoveToEx(hdc, x, y, None);
            } else {
                let _ = LineTo(hdc, x, y);
            }
        }
    }
}

fn fill_rect(hdc: HDC, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(color);
        let sel = SelectObject(hdc, b);
        let _ = Rectangle(hdc, x, y, x + w, y + h);
        let _ = SelectObject(hdc, sel);
        let _ = DeleteObject(HGDIOBJ(b.0 as *mut _));
    }
}

// 眼睛徽章：嵌入的 32bpp BMP（从参照 exe 切出的眼睛+双环，188×188），GDI 解析
const EMBLEM_BMP: &[u8] = include_bytes!("assets/emblem.bmp");

/// 解析嵌入的 32bpp BMP → HBITMAP（CreateDIBSection，top-down 负高度）
fn load_emblem() -> HBITMAP {
    unsafe {
        let d = EMBLEM_BMP;
        if d.len() < 54 || &d[0..2] != b"BM" {
            return HBITMAP::default();
        }
        let offset = u32::from_le_bytes(d[10..14].try_into().unwrap_or([0u8; 4])) as usize;
        let w = i32::from_le_bytes(d[18..22].try_into().unwrap_or([0u8; 4])).abs() as u32;
        let h = i32::from_le_bytes(d[22..26].try_into().unwrap_or([0u8; 4])).abs() as u32;
        let bitcount = u16::from_le_bytes(d[28..30].try_into().unwrap_or([0u8; 2]));
        if w == 0 || h == 0 || bitcount != 32 {
            return HBITMAP::default();
        }
        if offset + (w * h * 4) as usize > d.len() {
            return HBITMAP::default();
        }
        let src_top_down = i32::from_le_bytes(d[22..26].try_into().unwrap_or([0u8; 4])) < 0;
        let stride = w * 4;

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = 40;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32); // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0;

        let mut ppv: *mut core::ffi::c_void = core::ptr::null_mut();
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return HBITMAP::default();
        }
        let bm = match CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut ppv, None, 0) {
            Ok(bm) => bm,
            Err(_) => {
                let _ = DeleteDC(hdc);
                return HBITMAP::default();
            }
        };
        let _ = DeleteDC(hdc);
        if ppv.is_null() {
            return HBITMAP::default();
        }
        // 把 BMP 像素写进 GDI 分配的内存（top-down：源行序归一化）
        let base = ppv as *mut u8;
        for row in 0..h {
            let src_row = if src_top_down { row } else { h - 1 - row };
            for col in 0..w {
                let si = offset + ((src_row * stride + col * 4) as usize);
                let di = (row * stride + col * 4) as usize;
                for k in 0..4 {
                    *base.add(di + k) = d[si + k];
                }
            }
        }
        bm
    }
}

/// 把 emblem（188×188）缩放到 EMB_SZ×EMB_SZ 贴到 (x,y)（HALFTONE 缩放 + 抗锯齿）
fn blit_emblem(hdc: HDC, a: &App, x: i32, y: i32) {
    if a.emblem.is_invalid() {
        return;
    }
    unsafe {
        let mem = CreateCompatibleDC(hdc);
        if mem.is_invalid() {
            return;
        }
        let old = SelectObject(mem, a.emblem);
        let _ = SetStretchBltMode(mem, STRETCH_HALFTONE);
        let _ = StretchBlt(
            hdc,
            x,
            y,
            EMB_SZ,
            EMB_SZ,
            mem,
            0,
            0,
            188,
            188,
            SRCCOPY,
        );
        let _ = SelectObject(mem, old);
        let _ = DeleteDC(mem);
    }
}

// ---------------- 主窗口绘制 ----------------
fn draw_main(a: &App, hdc: HDC, f_small: HFONT, f_med: HFONT, f_big: HFONT, f_pct: HFONT) {
    unsafe {
        fill_rect(hdc, 0, 0, WIN_W, WIN_H, MAIN_BG);
        fill_rect(hdc, 0, 0, SIDE_W, WIN_H, TEAL);

        // 侧栏三项：护眼/健康/关于（选中项深块 + 白色图标文字）
        let items: [(u8, &str); 3] = [(0, "护眼"), (1, "健康"), (2, "关于")];
        for (idx, label) in items {
            let (y0, y1) = match idx {
                0 => NAV_EYE,
                1 => NAV_HEALTH,
                _ => NAV_ABOUT,
            };
            let active = a.page == idx;
            if active {
                fill_rect(hdc, 0, y0, SIDE_W, y1 - y0, TEAL_DARK);
            }
            let icx = 24i32;
            let icy = (y0 + y1) / 2;
            let col_pen = WHITE;
            let oldp = pen_w(hdc, 2, col_pen);
            match idx {
                0 => {
                    let _ = Ellipse(hdc, icx - 6, icy - 5, icx + 6, icy + 5);
                    let _ = Ellipse(hdc, icx - 2, icy - 2, icx + 2, icy + 2);
                }
                1 => {
                    let _ = Ellipse(hdc, icx - 5, icy - 5, icx + 5, icy + 5);
                    draw_arc_poly(hdc, icx, icy + 1, 3, 20.0, 140.0);
                }
                _ => {
                    let _ = Ellipse(hdc, icx - 3, icy - 6, icx + 3, icy - 1);
                    let _ = Ellipse(hdc, icx - 5, icy + 1, icx + 5, icy + 7);
                }
            }
            let _ = oldp;
            let tcol = WHITE;
            let t = u16s(label);
            let _ = SetTextColor(hdc, tcol);
            let _ = SetBkMode(hdc, TRANSPARENT);
            let oldf = SelectObject(hdc, f_med);
            let _ = TextOutW(hdc, 38, icy - 6, &t);
            let _ = SelectObject(hdc, oldf);
        }

        // 右上角 ☰ / ✕
        let oldp = pen_w(hdc, 2, TEXT_DARK);
        for i in 0..3 {
            let yy = TOP_Y0 + 4 + i * 5;
            let _ = MoveToEx(hdc, MENU_X0 + 2, yy, None);
            let _ = LineTo(hdc, MENU_X1 - 2, yy);
        }
        let _ = MoveToEx(hdc, CLOSE_X0 + 2, TOP_Y0 + 4, None);
        let _ = LineTo(hdc, CLOSE_X1 - 2, TOP_Y1 - 4);
        let _ = MoveToEx(hdc, CLOSE_X0 + 2, TOP_Y1 - 4, None);
        let _ = LineTo(hdc, CLOSE_X1 - 2, TOP_Y0 + 4);
        let _ = oldp;

        match a.page {
            0 => draw_page_eye(a, hdc, f_small, f_med, f_pct),
            1 => draw_page_health(a, hdc, f_small, f_med),
            _ => draw_page_about(a, hdc, f_small, f_med),
        }

        for f in [f_small, f_med, f_big, f_pct] {
            if !f.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(f.0 as *mut _));
            }
        }
    }
}

fn draw_page_eye(a: &App, hdc: HDC, f_small: HFONT, _f_med: HFONT, _f_pct: HFONT) {
    unsafe {
        // 眼睛徽章（嵌入 BMP 贴图，抗锯齿缩放）
        blit_emblem(hdc, a, EMB_X, EMB_Y);

        // 状态文字（teal，前置小点 + 下划线，贴合参照）
        let active = is_tinting(a);
        let st = status_text(a);
        let st_col = if active { TEAL } else { TEXT_GRAY };
        let st_w = text_width(hdc, f_small, st);
        let st_x = CX - st_w / 2;
        text_center(hdc, f_small, CX, STATUS_Y, st, st_col);
        // 前置圆点
        let dotx = st_x - 10;
        let ob = CreateSolidBrush(st_col);
        let sb = SelectObject(hdc, ob);
        let _ = Ellipse(hdc, dotx, STATUS_Y + 6, dotx + 6, STATUS_Y + 12);
        let _ = SelectObject(hdc, sb);
        let _ = DeleteObject(HGDIOBJ(ob.0 as *mut _));
        // 下划线
        let oldp = pen_w(hdc, 1, st_col);
        let _ = MoveToEx(hdc, st_x, STATUS_Y + 17, None);
        let _ = LineTo(hdc, st_x + st_w, STATUS_Y + 17);
        let _ = oldp;

        // 滑条（青→绿→黄渐变）
        let val = a.cfg.intensity as i32;
        let total = SL_X1 - SL_X0;
        let handle_x = SL_X0 + total * val / 100;
        let c0 = TEAL.0;
        let c1 = SL_YEL.0;
        let th = 5i32; // 轨道高度
        for px in 0..total {
            let x = SL_X0 + px;
            let filled = px <= total * val / 100;
            let col = if filled {
                let frac = if total > 0 {
                    px as f32 / total as f32
                } else {
                    0.0
                };
                COLORREF(lerp_color(c0, c1, frac.clamp(0.0, 1.0)))
            } else {
                LINE_GRAY
            };
            let ob = CreateSolidBrush(col);
            let sb = SelectObject(hdc, ob);
            let _ = Rectangle(hdc, x, SL_Y - th / 2, x + 1, SL_Y + th / 2 + 1);
            let _ = SelectObject(hdc, sb);
            let _ = DeleteObject(HGDIOBJ(ob.0 as *mut _));
        }
        // 手柄
        let ob = CreateSolidBrush(WHITE);
        let sb = SelectObject(hdc, ob);
        let oldp = pen_w(hdc, 1, LINE_GRAY);
        let _ = Ellipse(hdc, handle_x - 6, SL_Y - 6, handle_x + 6, SL_Y + 6);
        let _ = oldp;
        let _ = SelectObject(hdc, sb);
        let _ = DeleteObject(HGDIOBJ(ob.0 as *mut _));

        // 标签 + 夜间开关
        draw_text(hdc, f_small, SL_X0, LABEL_Y, "颜色调节", TEXT_DARK);
        draw_text(hdc, f_small, TOG_LBL_X, LABEL_Y, "夜间助眠", TEXT_DARK);
        draw_toggle(hdc, TOG_X, TOG_Y, a.cfg.auto_time);

        // 五个色圆
        let locked = a.cfg.auto_time;
        for i in 0..5 {
            let cx = CIRCLE_XS[i];
            let col = if locked && i != (a.cfg.temp.min(4) as usize) {
                COLORREF(0x00D8D8D8)
            } else {
                CIRCLE_COLORS[i]
            };
            let ob = CreateSolidBrush(col);
            let sb = SelectObject(hdc, ob);
            let ringcol = if a.cfg.temp == i as u8 && !locked { TEAL } else { LINE_GRAY };
            let oldp2 =
                pen_w(hdc, if a.cfg.temp == i as u8 && !locked { 3 } else { 1 }, ringcol);
            let _ = Ellipse(
                hdc,
                cx - CIRCLE_R,
                CIRCLE_Y - CIRCLE_R,
                cx + CIRCLE_R,
                CIRCLE_Y + CIRCLE_R,
            );
            let _ = oldp2;
            let _ = SelectObject(hdc, sb);
            let _ = DeleteObject(HGDIOBJ(ob.0 as *mut _));
        }

        let cap = if a.cfg.auto_time {
            "已开启夜间助眠，将在设定时段自动护眼"
        } else {
            "点击圆点切换护眼颜色，黑色为关闭"
        };
        text_center(hdc, f_small, CX, CAPTION_Y, cap, TEXT_GRAY);
    }
}

fn draw_toggle(hdc: HDC, x: i32, y: i32, on: bool) {
    unsafe {
        let knob = TOG_H - 2;
        let bg = if on { TEAL } else { LINE_GRAY };
        let ob = CreateSolidBrush(bg);
        let sb = SelectObject(hdc, ob);
        let _ = RoundRect(hdc, x, y, x + TOG_W, y + TOG_H, TOG_H, TOG_H);
        let _ = SelectObject(hdc, sb);
        let _ = DeleteObject(HGDIOBJ(ob.0 as *mut _));
        let knobx = if on { x + TOG_W - knob - 1 } else { x + 1 };
        let ob2 = CreateSolidBrush(WHITE);
        let sb2 = SelectObject(hdc, ob2);
        let _ = Ellipse(hdc, knobx, y + 1, knobx + knob, y + TOG_H - 1);
        let _ = SelectObject(hdc, sb2);
        let _ = DeleteObject(HGDIOBJ(ob2.0 as *mut _));
    }
}

fn draw_value_badge(hdc: HDC, text: &str, x: i32, y: i32, f_small: HFONT) {
    unsafe {
        let w = text_width(hdc, f_small, text) + 20;
        let ob = CreateSolidBrush(COLORREF(0x00C8E8F5)); // #F5E8C8 暖黄底
        let sb = SelectObject(hdc, ob);
        let _ = RoundRect(hdc, x, y - 3, x + w, y + 19, 11, 11);
        let _ = SelectObject(hdc, sb);
        let _ = DeleteObject(HGDIOBJ(ob.0 as *mut _));
        draw_text(hdc, f_small, x + 10, y, text, TEXT_DARK);
    }
}

fn draw_page_health(a: &App, hdc: HDC, f_small: HFONT, f_med: HFONT) {
    draw_text(hdc, f_med, 100, 40, "健康 · 休息提醒", TEXT_DARK);
    draw_text(hdc, f_med, 100, HP_ROW1_Y, "休息提醒（20-20-20）", TEXT_DARK);
    draw_toggle(hdc, HP_VAL_X, HP_ROW1_Y - 4, a.cfg.auto_break);

    draw_text(hdc, f_med, 100, HP_ROW2_Y, "休息时长", TEXT_DARK);
    let v = format!("{} 分钟", a.cfg.rest_min);
    draw_value_badge(hdc, &v, HP_VAL_X, HP_ROW2_Y, f_small);

    draw_text(hdc, f_med, 100, HP_ROW3_Y, "专注阈值（无输入多久提醒）", TEXT_DARK);
    let v2 = format!("{} 分钟", a.cfg.focus_min);
    draw_value_badge(hdc, &v2, HP_VAL_X, HP_ROW3_Y, f_small);

    text_center(
        hdc,
        f_small,
        CX,
        175,
        "点击数值可循环切换：休息 5/10/20/30/60 分钟，阈值 10/20/30/45/60 分钟",
        TEXT_GRAY,
    );
    text_center(
        hdc,
        f_small,
        CX,
        192,
        "提醒时出现全屏倒计时，建议闭眼或远眺 20 秒",
        TEXT_GRAY,
    );
}

fn draw_page_about(a: &App, hdc: HDC, f_small: HFONT, f_med: HFONT) {
    let _ = a;
    draw_text(hdc, f_med, 100, 46, "sdz-ce 0.1.0", TEXT_DARK);
    let lines = [
        "护眼原理：DWM 暖色调色层（全屏点击穿透分层窗），",
        "不修改系统 gamma 表，进程退出即恢复，零残留。",
        "",
        "热键：F9 开/关护眼　F8 立即休息　F5 打开本窗口",
        "配置：%APPDATA%\\sdz-ce\\config.ini（纯文本，可手改）",
    ];
    for (i, l) in lines.iter().enumerate() {
        draw_text(hdc, f_small, 100, 80 + i as i32 * 16, l, TEXT_GRAY);
    }
    text_center(hdc, f_small, CX, 190, "基于 Windows 10/11，普通用户权限运行", TEXT_GRAY);
}

// ---------------- 主窗口命中 ----------------
fn hit_main(a: &App, x: i32, y: i32) -> u32 {
    if x >= MENU_X0 && x <= MENU_X1 && y >= TOP_Y0 && y <= TOP_Y1 {
        return HIT_MENU;
    }
    if x >= CLOSE_X0 && x <= CLOSE_X1 && y >= TOP_Y0 && y <= TOP_Y1 {
        return HIT_CLOSE;
    }
    if x < SIDE_W {
        return match y {
            y if (y >= NAV_EYE.0 && y < NAV_EYE.1) => HIT_PAGE_EYE,
            y if (y >= NAV_HEALTH.0 && y < NAV_HEALTH.1) => HIT_PAGE_HEALTH,
            y if (y >= NAV_ABOUT.0 && y < NAV_ABOUT.1) => HIT_PAGE_ABOUT,
            _ => HIT_NONE,
        };
    }
    match a.page {
        0 => {
            if y >= SL_Y - 12 && y <= SL_Y + 12 && x >= SL_X0 - 12 && x <= SL_X1 + 12 {
                return HIT_SLIDER;
            }
            for i in 0..5 {
                let cx = CIRCLE_XS[i];
                if (x - cx).abs() <= CIRCLE_R + 4 && (y - CIRCLE_Y).abs() <= CIRCLE_R + 4 {
                    return HIT_CIRC_BASE + i as u32;
                }
            }
            if x >= TOG_X - 6
                && x <= TOG_X + TOG_W + 6
                && y >= TOG_Y - 6
                && y <= TOG_Y + TOG_H + 6
            {
                return HIT_TOG_NIGHT;
            }
        }
        1 => {
            if x >= HP_VAL_X - 4 && x <= HP_VAL_X + TOG_W + 4 && y >= HP_ROW1_Y - 6 && y <= HP_ROW1_Y + 12 {
                return HIT_TOG_BREAK;
            }
            if x >= HP_VAL_X - 4 && x <= HP_VAL_X + 90 && y >= HP_ROW2_Y - 6 && y <= HP_ROW2_Y + 20 {
                return HIT_REST;
            }
            if x >= HP_VAL_X - 4 && x <= HP_VAL_X + 90 && y >= HP_ROW3_Y - 6 && y <= HP_ROW3_Y + 20 {
                return HIT_FOCUS;
            }
        }
        _ => {}
    }
    HIT_NONE
}

fn slider_value_from_x(x: i32) -> u8 {
    let total = (SL_X1 - SL_X0) as f32;
    if total <= 0.0 {
        return 0;
    }
    let t = ((x - SL_X0) as f32 / total).clamp(0.0, 1.0);
    (t * 100.0).round() as u8
}

// ---------------- 主窗口 WndProc ----------------
unsafe extern "system" fn wnd_null(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

unsafe extern "system" fn wnd_main(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let a = &mut *APP;
    match m {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(h, &mut ps);
            let f_small = font_seg_ui(9, 400);
            let f_med = font_seg_ui(11, 500);
            let f_big = font_seg_ui(14, 600);
            let f_pct = font_seg_ui(15, 700);
            draw_main(a, hdc, f_small, f_med, f_big, f_pct);
            let _ = ReleaseDC(h, hdc);
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (l.0 & 0xFFFF) as i16 as i32;
            let y = ((l.0 >> 16) & 0xFFFF) as i16 as i32;
            let hit = hit_main(a, x, y);
            match hit {
                HIT_NONE => {
                    // 无边框窗口：没命中任何控件的空白处按下 → 屏幕坐标抓取式拖动 + 捕获鼠标
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let mut r = RECT::default();
                    GetWindowRect(h, &mut r).ok();
                    a.drag = Some((pt.x - r.left, pt.y - r.top));
                    // 捕获鼠标：即使拖出窗口外，松开时 LBUTTONUP 也保证送达本窗口，拖拽标记不会残留
                    let _ = SetCapture(h);
                    LRESULT(0)
                }
                HIT_PAGE_EYE => {
                    a.page = 0;
                    refresh_main(a);
                    LRESULT(0)
                }
                HIT_PAGE_HEALTH => {
                    a.page = 1;
                    refresh_main(a);
                    LRESULT(0)
                }
                HIT_PAGE_ABOUT => {
                    a.page = 2;
                    refresh_main(a);
                    LRESULT(0)
                }
                HIT_SLIDER => {
                    a.mouse_down = true;
                    a.cfg.intensity = slider_value_from_x(x);
                    // 处于"关闭"状态时调滑条 → 自动切回默认暖色
                    if a.cfg.temp == 4 {
                        a.cfg.temp = 1;
                    }
                    apply_tint(a);
                    save_cfg(&a.cfg);
                    refresh_main(a);
                    LRESULT(0)
                }
                _ => {
                    if hit >= HIT_CIRC_BASE && hit < HIT_CIRC_BASE + 5 {
                        let idx = (hit - HIT_CIRC_BASE) as u8;
                        if !a.cfg.auto_time {
                            a.cfg.temp = idx;
                            apply_tint(a);
                            save_cfg(&a.cfg);
                        }
                    } else if hit == HIT_TOG_NIGHT {
                        a.cfg.auto_time = !a.cfg.auto_time;
                        // 处于"关闭"（黑点）时开启夜间助眠 → 切回黄色，保证有可见效果
                        if a.cfg.auto_time && a.cfg.temp == 4 {
                            a.cfg.temp = 1;
                        }
                        apply_tint(a);
                        save_cfg(&a.cfg);
                    } else if hit == HIT_MENU {
                        main_menu(a);
                        return LRESULT(0);
                    } else if hit == HIT_CLOSE {
                        let _ = ShowWindow(h, SW_HIDE);
                        return LRESULT(0);
                    } else if hit == HIT_TOG_BREAK {
                        a.cfg.auto_break = !a.cfg.auto_break;
                        save_cfg(&a.cfg);
                    } else if hit == HIT_REST {
                        let cur = a.cfg.rest_min;
                        let ni = REST_VALUES.iter().position(|v| *v == cur).unwrap_or(2) + 1;
                        a.cfg.rest_min = REST_VALUES[ni % REST_VALUES.len()];
                        save_cfg(&a.cfg);
                    } else if hit == HIT_FOCUS {
                        let cur = a.cfg.focus_min;
                        let ni = FOCUS_VALUES.iter().position(|v| *v == cur).unwrap_or(1) + 1;
                        a.cfg.focus_min = FOCUS_VALUES[ni % FOCUS_VALUES.len()];
                        save_cfg(&a.cfg);
                    }
                    refresh_main(a);
                    LRESULT(0)
                }
            }
        }
        WM_MOUSEMOVE => {
            if a.mouse_down {
                let x = (l.0 & 0xFFFF) as i16 as i32;
                let nv = slider_value_from_x(x);
                if nv != a.cfg.intensity {
                    a.cfg.intensity = nv;
                    if a.cfg.temp == 4 {
                        a.cfg.temp = 1;
                    }
                    apply_tint(a);
                    save_cfg(&a.cfg);
                    refresh_main(a);
                }
            }
            if let Some((ox, oy)) = a.drag {
                // 屏幕坐标抓取式拖动：窗口位置 = 光标 - 按下时偏移，无累积、无正反馈
                let mut pt = POINT::default();
                if GetCursorPos(&mut pt).is_ok() {
                    let _ = SetWindowPos(
                        h,
                        HWND_TOP,
                        pt.x - ox,
                        pt.y - oy,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            a.mouse_down = false;
            if a.drag.is_some() {
                let _ = ReleaseCapture();
            }
            a.drag = None;
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // 捕获被系统收回（切焦点/Alt+F4 等）时，清掉拖动标记，防止残留
            if a.drag.is_some() {
                a.drag = None;
            }
            DefWindowProcW(h, m, w, l)
        }
        WM_CLOSE => {
            let _ = ShowWindow(h, SW_HIDE);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

// ---------------- 主窗口右上角菜单 ----------------
fn main_menu(a: &mut App) {
    unsafe {
        let m = match CreatePopupMenu().ok() {
            Some(m) => m,
            None => return,
        };
        let ins = |id: u32, text: &str, check: bool| {
            let t = u16z(text);
            let flags = MENU_ITEM_FLAGS(MF_STRING.0 | if check { MF_CHECKED.0 } else { 0 });
            let _ = AppendMenuW(m, flags, id as usize, PCWSTR(t.as_ptr()));
        };
        ins(CM_AUTOSTART, "开机自启", a.cfg.autostart);
        ins(CM_SYSNIGHT, "打开系统夜灯", false);
        ins(CM_EXIT, "退出 sdz-ce", false);
        let _ = SetForegroundWindow(a.main_w);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let r = TrackPopupMenu(
            m,
            TRACK_POPUP_MENU_FLAGS(TPM_RIGHTBUTTON.0 | TPM_RETURNCMD.0),
            pt.x,
            pt.y,
            0,
            a.main_w,
            None,
        );
        let _ = DestroyMenu(m);
        if r.0 != 0 {
            match r.0 as u32 {
                CM_AUTOSTART => {
                    a.cfg.autostart = !a.cfg.autostart;
                    apply_autostart(a.cfg.autostart);
                    save_cfg(&a.cfg);
                }
                CM_SYSNIGHT => {
                    let u16 = "ms-settings:nightlight".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
                    let _ = ShellExecuteW(
                        a.main_w,
                        PCWSTR("open".encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>()[..].as_ptr()),
                        PCWSTR(u16.as_ptr()),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
                CM_EXIT => {
                    save_cfg(&a.cfg);
                    let _ = PostMessageW(a.hidden, WM_DESTROY, WPARAM(0), LPARAM(0));
                    return;
                }
                _ => {}
            }
            refresh_main(a);
        }
    }
}

// ---------------- 隐藏消息窗 ----------------
unsafe extern "system" fn wnd_hidden(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_TIMER => {
            let a = &mut *APP;
            match w.0 as u32 {
                TIMER_IDLE => tick_idle(a),
                TIMER_HUD => hud_tick(a),
                _ => {}
            }
            LRESULT(0)
        }
        WM_HOTKEY => {
            let a = &mut *APP;
            match w.0 as i32 {
                HK_TOGGLE => {
                    if is_tinting(a) {
                        a.cfg.intensity = 0;
                    } else {
                        if a.cfg.intensity == 0 {
                            a.cfg.intensity = 40;
                        }
                        a.cfg.temp = a.cfg.temp.min(3);
                    }
                    apply_tint(a);
                    save_cfg(&a.cfg);
                    refresh_main(a);
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
                }
                HK_BREAK => {
                    start_break(a, (a.cfg.rest_min as u32) * 60);
                }
                HK_SHOW => {
                    let hv = a.main_w;
                    if !hv.is_invalid() && IsWindow(hv).0 != 0 {
                        let _ = ShowWindow(hv, SW_SHOWNORMAL);
                        let _ = SetForegroundWindow(hv);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        c if c == TRAY_CALLBACK => {
            let a = &mut *APP;
            let ev = l.0 as u32 & 0xFFFF;
            match ev {
                EVENT_LEFTBUTTONUP => {
                    let hv = a.main_w;
                    let _ = ShowWindow(hv, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(hv);
                }
                EVENT_RIGHTBUTTONUP => {
                    tray_menu(a);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let a = &mut *APP;
            KillTimer(a.hidden, TIMER_IDLE as usize).ok();
            KillTimer(a.hidden, TIMER_HUD as usize).ok();
            let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(a));
            let _ = PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

// ---------------- 托盘右键菜单 ----------------
fn tray_menu(a: &mut App) {
    unsafe {
        let m = match CreatePopupMenu().ok() {
            Some(m) => m,
            None => return,
        };
        const MI_SHOW: u32 = 1;
        const MI_TOG: u32 = 2;
        const MI_BREAK: u32 = 3;
        const MI_NIGHT: u32 = 4;
        const MI_BOOT: u32 = 5;
        const MI_ABOUT: u32 = 6;
        const MI_EXIT: u32 = 7;
        let ins = |id: u32, text: &str| {
            let t = u16z(text);
            let _ = AppendMenuW(m, MENU_ITEM_FLAGS(MF_STRING.0), id as usize, PCWSTR(t.as_ptr()));
        };
        ins(MI_SHOW, "打开主窗口");
        ins(MI_TOG, if is_tinting(a) { "关闭护眼" } else { "开启护眼" });
        ins(MI_BREAK, "立即休息");
        ins(MI_NIGHT, if a.cfg.auto_time { "夜间助眠：开" } else { "夜间助眠：关" });
        ins(MI_BOOT, if a.cfg.autostart { "开机自启：开" } else { "开机自启：关" });
        ins(MI_ABOUT, "关于");
        let _ = AppendMenuW(m, MENU_ITEM_FLAGS(MF_SEPARATOR.0), 0, PCWSTR(std::ptr::null()));
        ins(MI_EXIT, "退出 sdz-ce");
        let _ = SetForegroundWindow(a.hidden);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let r = TrackPopupMenu(
            m,
            TRACK_POPUP_MENU_FLAGS(TPM_RIGHTBUTTON.0 | TPM_RETURNCMD.0),
            pt.x,
            pt.y,
            0,
            a.hidden,
            None,
        );
        let _ = DestroyMenu(m);
        if r.0 != 0 {
            match r.0 as u32 {
                MI_SHOW => {
                    let hv = a.main_w;
                    let _ = ShowWindow(hv, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(hv);
                }
                MI_TOG => {
                    if is_tinting(a) {
                        a.cfg.intensity = 0;
                    } else {
                        if a.cfg.intensity == 0 {
                            a.cfg.intensity = 40;
                        }
                        a.cfg.temp = a.cfg.temp.min(3);
                    }
                    apply_tint(a);
                    save_cfg(&a.cfg);
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
                }
                MI_BREAK => {
                    start_break(a, (a.cfg.rest_min as u32) * 60);
                }
                MI_NIGHT => {
                    a.cfg.auto_time = !a.cfg.auto_time;
                    apply_tint(a);
                    save_cfg(&a.cfg);
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
                }
                MI_BOOT => {
                    a.cfg.autostart = !a.cfg.autostart;
                    apply_autostart(a.cfg.autostart);
                    save_cfg(&a.cfg);
                }
                MI_ABOUT => {
                    let msg = u16z(
                        "sdz-ce 0.1.0\n护眼：DWM 暖色调色层（不修改系统 gamma，零残留）\n休息：20-20-20 提醒\n\n热键：F9 开/关　F8 休息　F5 主窗口",
                    );
                    let cap = u16z("关于 sdz-ce");
                    MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(cap.as_ptr()), MB_OK);
                }
                MI_EXIT => {
                    save_cfg(&a.cfg);
                    let _ = PostMessageW(a.hidden, WM_DESTROY, WPARAM(0), LPARAM(0));
                }
                _ => {}
            }
        }
    }
}

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// 带 NUL 结尾的 UTF-16 字符串，专供需要 PCWSTR（以 \0 结尾）的 Win32 API 使用
fn u16z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------- 创建主窗口 ----------------
fn create_main(a: &mut App) {
    unsafe {
        let cls = CLASS_MAIN.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_main),
            hInstance: hinst(),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let title = u16z("sdz-ce 护眼");
        let x = (GetSystemMetrics(SM_CXSCREEN) - WIN_W) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - WIN_H) / 3;
        a.main_w = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(WS_POPUP.0 | WS_CLIPSIBLINGS.0),
            x,
            y,
            WIN_W,
            WIN_H,
            None,
            None,
            hinst(),
            None,
        )
        .unwrap_or_default();
        if !a.main_w.is_invalid() {
            // Win11 无边框圆角
            let pref = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                a.main_w,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const DWM_WINDOW_CORNER_PREFERENCE as *const core::ffi::c_void,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
            let _ = ShowWindow(a.main_w, SW_SHOWNORMAL);
        }
    }
}

// ---------------- 入口 ----------------
fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).ok();

        // 单实例（NUL 结尾 + 句柄常驻到进程结束）
        let name: Vec<u16> = "Local\\sdz-ce_mutex"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if let Ok(m) = CreateMutexW(None, TRUE, PCWSTR(name.as_ptr())) {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let msg = u16z("sdz-ce 已在运行：请查看任务栏右下角托盘图标（点 ^ 展开），或按 F5 打开窗口。");
                let cap = u16z("sdz-ce");
                MessageBoxW(None, PCWSTR(msg.as_ptr()), PCWSTR(cap.as_ptr()), MB_OK);
                return;
            }
            MUTEX = m;
        }

        let mut app = App {
            hidden: HWND::default(),
            tint: HWND::default(),
            hud: HWND::default(),
            main_w: HWND::default(),
            emblem: load_emblem(),
            cfg: load_cfg(),
            page: 0,
            in_break: false,
            hud_left: 0,
            mouse_down: false,
            drag: None,
        };

        let cls = CLASS_HIDDEN.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_hidden),
            hInstance: hinst(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        app.hidden = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            None,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            hinst(),
            None,
        )
        .unwrap_or_default();

        APP = Box::leak(Box::new(app)) as *mut App;
        let a = &mut *APP;

        create_tint(a);
        create_hud(a);
        create_main(a);

        let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(a));
        SetTimer(a.hidden, TIMER_IDLE as usize, 10_000, None);

        RegisterHotKey(a.hidden, HK_TOGGLE, MOD_NOREPEAT, 0x78).ok(); // F9
        RegisterHotKey(a.hidden, HK_BREAK, MOD_NOREPEAT, 0x77).ok();  // F8
        RegisterHotKey(a.hidden, HK_SHOW, MOD_NOREPEAT, 0x74).ok();   // F5

        if a.cfg.autostart {
            apply_autostart(true);
        }

        // 消息循环
        let mut msg = MSG::default();
        loop {
            let got = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if got.0 != 0 {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            } else {
                break;
            }
        }

        let a = &mut *APP;
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(a));
        let _ = DestroyWindow(a.tint);
        let _ = DestroyWindow(a.hud);
        let _ = DestroyWindow(a.main_w);
        if !a.emblem.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(a.emblem.0 as *mut _));
        }
    }
}
