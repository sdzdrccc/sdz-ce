//! sdz-ce：轻量护眼工具
//!
//! 护眼新方案（不修改系统 gamma 查找表，区别于"护眼宝"类旧工具）：
//!   用 DWM 分层窗（WS_EX_LAYERED + SetLayeredWindowAttributes/LWA_ALPHA）
//!   覆盖全虚拟屏幕（含多显示器），叠加半透明暖色层，等效压低蓝光占比、柔化屏幕。
//!   颜色由系统 DWM 渲染托管：进程退出或被杀，分层窗自动移除，屏幕瞬间恢复，零残留。
//!
//! 配套功能：
//!   - 20-20-20 休息提醒（GetLastInputInfo 空闲检测 + 置顶大字倒计时 + 响铃）
//!   - 系统托盘（Shell_NotifyIconW：左键开/关护眼，右键菜单）
//!   - 全局热键：F9 开/关护眼，F8 立即休息，F5 打开设置
//!   - 按时段自动开关护眼（可选）
//!   - 开机自启（HKCU\...\Run，可选）
//!   - 配置存 %APPDATA%\sdz-ce\config.ini

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::Diagnostics::Debug::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------- 常量 ----------------

const CLASS_HIDDEN: &str = "sdz_ce_hidden";
const CLASS_TINT: &str = "sdz_ce_tint";
const CLASS_HUD: &str = "sdz_ce_hud";
const CLASS_SET: &str = "sdz_ce_set";

const TRAY_ID: u32 = 0xC3E;
const TRAY_CALLBACK: u32 = WM_USER + 1;
const TIMER_IDLE: u32 = 1; // 每 10s：空闲检测 + 自动时段检查
const TIMER_HUD: u32 = 2;  // 每 1s：休息倒计时

const HK_TOGGLE: i32 = 1; // F9
const HK_BREAK: i32 = 2;  // F8
const HK_SET: i32 = 3;    // F5

// 托盘右键菜单命令
const MI_TOGGLE: u32 = 1000;
const MI_P0: u32 = 1001;
const MI_P1: u32 = 1002;
const MI_P2: u32 = 1003;
const MI_T0: u32 = 1004;
const MI_T1: u32 = 1005;
const MI_T2: u32 = 1006;
const MI_T3: u32 = 1007;
const MI_REST5: u32 = 1008;
const MI_REST10: u32 = 1009;
const MI_REST20: u32 = 1010;
const MI_AUTOBREAK: u32 = 1011;
const MI_AUTOTIME: u32 = 1012;
const MI_BREAKNOW: u32 = 1013;
const MI_BOOT: u32 = 1014;
const MI_ABOUT: u32 = 1015;
const MI_EXIT: u32 = 1016;

// 设置窗控件 ID
const SL_PRESET: u32 = 2001;
const SL_TEMP: u32 = 2002;
const SL_REST: u32 = 2003;
const SL_FOCUS: u32 = 2004;
const SL_ONH: u32 = 2005;
const SL_ONM: u32 = 2006;
const SL_OFFH: u32 = 2007;
const SL_OFFM: u32 = 2008;
const CK_2020: u32 = 3001;
const CK_AUTO: u32 = 3002;
const CK_BOOT: u32 = 3003;
const BT_OK: u32 = 4001;
const BT_CANCEL: u32 = 4002;
const HUD_TEXT: u32 = 9001;

// 标准控件消息（数值常量，免 Win32_UI_Controls feature）
const TBM_SETMINPOS: u32 = 0x040E;
const TBM_SETMAXPOS: u32 = 0x040F;
const TBM_SETPOS: u32 = 0x0402;
const TBM_GETPOS: u32 = 0x0405;
const BM_SETCHECK: u32 = 0x0F01;
const BM_GETCHECK: u32 = 0x0F00;

// 托盘事件（NOTIFYICONIDENTIFIER 低 16 位）
const EVENT_LEFTBUTTONUP: u32 = 0x0402;
const EVENT_RIGHTBUTTONUP: u32 = 0x0405;

// 色温档位（BGR 暖色，alpha 由强度档决定；索引 0 最冷 -> 索引 3 最暖）
const TEMPS: [u32; 4] = [0x0080B0FF, 0x0088C4FF, 0x0090D4FF, 0x00A0E8FF];
const TEMP_NAMES: [&str; 4] = ["偏冷", "中性", "偏暖", "暖"];
const PRESET_NAMES: [&str; 3] = ["标准", "偏低", "偏高"];
const PRESET_ALPHAS: [u8; 3] = [40, 65, 90];

// ---------------- 配置 ----------------

#[derive(Clone, Copy)]
struct Cfg {
    preset: u8,    // 0 标准 / 1 偏低 / 2 偏高 / 3 关闭
    temp: u8,      // 0..3 色温档
    rest_min: u8,  // 休息时长（分钟）
    focus_min: u8, // 专注阈值（分钟），空闲达到后自动提醒休息
    auto_break: bool,
    auto_time: bool,
    autostart: bool,
    auto_on_hm: u16,  // 自动开启时刻（分钟 = h*60+m）
    auto_off_hm: u16,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            preset: 0,
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
    let mut c = Cfg::default();
    if let Ok(t) = std::fs::read_to_string(cfg_path()) {
        for line in t.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                match k.trim() {
                    "preset" => c.preset = v.parse().unwrap_or(0).min(3),
                    "temp" => c.temp = v.parse().unwrap_or(1).min(3),
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
        "[sdz-ce]\npreset={}\ntemp={}\nrest_min={}\nfocus_min={}\n\
         auto_break={}\nauto_time={}\nautostart={}\nauto_on_hm={}\nauto_off_hm={}\n",
        c.preset, c.temp, c.rest_min, c.focus_min,
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
    cfg: Cfg,
    in_break: bool,
    hud_left: u32,
}

static mut APP: *mut App = ptr::null_mut();

#[allow(static_mut_refs)]

// ---------------- 开机自启（注册表 Run 键） ----------------

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
        .collect::<Vec<_>>();
    let name = "sdz-ce".encode_utf16().collect::<Vec<_>>();
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

fn is_active(a: &App) -> bool {
    if a.cfg.preset >= 3 {
        return false;
    }
    if a.cfg.auto_time {
        in_auto_window(&a.cfg)
    } else {
        true
    }
}

// ---------------- 托盘 ----------------

fn tray_tip(a: &App) -> String {
    if a.in_break {
        format!("sdz-ce：休息中 {}:{:02}", a.hud_left / 60, a.hud_left % 60)
    } else if is_active(a) {
        format!(
            "sdz-ce：护眼中 · {} · {}",
            PRESET_NAMES[a.cfg.preset as usize % 3],
            TEMP_NAMES[a.cfg.temp as usize % 4]
        )
    } else {
        "sdz-ce：已关闭".into()
    }
}

fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

fn tray_data(a: &App) -> NOTIFYICONDATAW {
    let tip = tray_tip(a);
    let t: Vec<u16> = tip.encode_utf16().collect();
    let icon = unsafe {
        LoadImageW(None, IDC_ARROW, IMAGE_ICON, 16, 16, LR_DEFAULTCOLOR)
            .ok()
            .map(|h| HICON(h.0))
            .unwrap_or_default()
    };
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK,
        hIcon: icon,
        ..Default::default()
    };
    d.hWnd = a.hidden;
    let n = t.len().min(127);
    d.szTip[..n].copy_from_slice(&t[..n]);
    d
}

// ---------------- 调色层（护眼核心） ----------------

fn apply_tint(a: &App) {
    let alpha = if is_active(a) {
        PRESET_ALPHAS[a.cfg.preset as usize % 3]
    } else {
        0
    };
    let color = COLORREF(TEMPS[a.cfg.temp as usize % 4]);
    unsafe {
        SetLayeredWindowAttributes(a.tint, color, alpha, LWA_ALPHA).ok();
    }
}

fn create_tint(a: &mut App) {
    unsafe {
        let mut cls = CLASS_TINT.encode_utf16().collect::<Vec<u16>>();
        cls.push(0);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_null),
            hInstance: hinst(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let vw = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vx = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_CYVIRTUALSCREEN);
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

// ---------------- 休息提示（置顶大字窗） ----------------

fn menu_id_h(id: u32) -> HMENU {
    HMENU(id as usize as *mut core::ffi::c_void)
}

fn set_hud_text(h: HWND, text: &str) {
    unsafe {
        let mut u16: Vec<u16> = text.encode_utf16().collect();
        u16.push(0);
        if let Ok(st) = GetDlgItem(h, HUD_TEXT as i32) {
            let _ = SendMessageW(
                st,
                WM_SETTEXT,
                WPARAM(u16.len()),
                LPARAM(u16.as_ptr() as isize),
            );
        }
    }
}

fn create_hud(a: &mut App) {
    unsafe {
        if !a.hud.is_invalid() {
            return;
        }
        let mut cls = CLASS_HUD.encode_utf16().collect::<Vec<u16>>();
        cls.push(0);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_null),
            hInstance: hinst(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let vw = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vx = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_CYVIRTUALSCREEN);
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
            PCWSTR("STATIC".encode_utf16().collect::<Vec<u16>>()[..].as_ptr()),
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
        let face = "Segoe UI".encode_utf16().collect::<Vec<u16>>();
        let f = CreateFontW(
            -42, 0, 0, 0, 700, 0, 0, 0,
            DEFAULT_CHARSET.0.into(), OUT_TT_PRECIS.0.into(), CLIP_DEFAULT_PRECIS.0.into(),
            CLEARTYPE_QUALITY.0.into(), (DEFAULT_PITCH.0 | FIXED_PITCH.0).into(),
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

// ---------------- 空闲检测 + 自动时段 ----------------

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
    }
}

// ---------------- 窗口过程 ----------------

unsafe extern "system" fn wnd_null(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

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
                HK_TOGGLE => command(a, MI_TOGGLE),
                HK_BREAK => start_break(a, (a.cfg.rest_min as u32) * 60),
                HK_SET => open_settings(a),
                _ => {}
            }
            LRESULT(0)
        }
        c if c == TRAY_CALLBACK => {
            let a = &mut *APP;
            let ev = l.0 as u32 & 0xFFFF; // NOTIFYICONIDENTIFIER 低 16 位
            match ev {
                EVENT_RIGHTBUTTONUP => tray_menu(a),
                EVENT_LEFTBUTTONUP => command(a, MI_TOGGLE),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let a = &mut *APP;
            KillTimer(a.hidden, TIMER_IDLE as usize).ok();
            KillTimer(a.hidden, TIMER_HUD as usize).ok();
            let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(a));
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

// ---------------- 托盘菜单 ----------------

fn tray_menu(a: &mut App) {
    unsafe {
        let m = match CreatePopupMenu().ok() {
            Some(m) => m,
            None => return,
        };
        let ins = |id: u32, text: &str, check: bool| {
            let t = text.encode_utf16().collect::<Vec<u16>>();
            let flags = MENU_ITEM_FLAGS(MF_STRING.0 | if check { MF_CHECKED.0 } else { 0 });
            AppendMenuW(m, flags, id as usize, PCWSTR(t.as_ptr())).ok();
        };
        let sep = || {
            AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR(ptr::null())).ok();
        };
        let active = is_active(a);
        ins(MI_TOGGLE, if active { "■ 关闭护眼" } else { "● 开启护眼" }, false);
        sep();
        ins(MI_P0, "强度：标准", a.cfg.preset == 0);
        ins(MI_P1, "强度：偏低", a.cfg.preset == 1);
        ins(MI_P2, "强度：偏高", a.cfg.preset == 2);
        sep();
        ins(MI_T0, "色温：偏冷", a.cfg.temp == 0);
        ins(MI_T1, "色温：中性", a.cfg.temp == 1);
        ins(MI_T2, "色温：偏暖", a.cfg.temp == 2);
        ins(MI_T3, "色温：暖", a.cfg.temp == 3);
        sep();
        ins(MI_REST5, "休息 5 分钟", a.cfg.rest_min == 5);
        ins(MI_REST10, "休息 10 分钟", a.cfg.rest_min == 10);
        ins(MI_REST20, "休息 20 分钟", a.cfg.rest_min == 20);
        ins(MI_AUTOBREAK, "休息提醒", a.cfg.auto_break);
        sep();
        ins(MI_AUTOTIME, "按时段自动开关", a.cfg.auto_time);
        ins(MI_BOOT, "开机自启", a.cfg.autostart);
        ins(MI_BREAKNOW, "立即休息", false);
        ins(MI_ABOUT, "关于", false);
        sep();
        ins(MI_EXIT, "退出", false);

        let _ = SetForegroundWindow(a.hidden);
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok();
        let r = TrackPopupMenu(
            m,
            TRACK_POPUP_MENU_FLAGS(TPM_RIGHTBUTTON.0 | TPM_RETURNCMD.0),
            pt.x,
            pt.y,
            0,
            a.hidden,
            None,
        );
        DestroyMenu(m).ok();
        if r.0 != 0 {
            let cmd = r.0 as u32;
            if cmd > 0 {
                command(a, cmd);
            }
        }
    }
}

fn command(a: &mut App, id: u32) {
    match id {
        MI_TOGGLE => a.cfg.preset = if a.cfg.preset < 3 { 3 } else { 0 },
        MI_P0 => a.cfg.preset = 0,
        MI_P1 => a.cfg.preset = 1,
        MI_P2 => a.cfg.preset = 2,
        MI_T0 => a.cfg.temp = 0,
        MI_T1 => a.cfg.temp = 1,
        MI_T2 => a.cfg.temp = 2,
        MI_T3 => a.cfg.temp = 3,
        MI_REST5 => a.cfg.rest_min = 5,
        MI_REST10 => a.cfg.rest_min = 10,
        MI_REST20 => a.cfg.rest_min = 20,
        MI_AUTOBREAK => a.cfg.auto_break = !a.cfg.auto_break,
        MI_AUTOTIME => a.cfg.auto_time = !a.cfg.auto_time,
        MI_BOOT => {
            a.cfg.autostart = !a.cfg.autostart;
            apply_autostart(a.cfg.autostart);
        }
        MI_BREAKNOW => {
            save_cfg(&a.cfg);
            start_break(a, (a.cfg.rest_min as u32) * 60);
            return;
        }
        MI_ABOUT => {
            menu_about();
            return;
        }
        MI_EXIT => {
            save_cfg(&a.cfg);
            unsafe {
                PostMessageW(a.hidden, WM_DESTROY, WPARAM(0), LPARAM(0)).ok();
            }
            return;
        }
        _ => return,
    }
    save_cfg(&a.cfg);
    apply_tint(a);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
    }
}

fn menu_about() {
    let text = u16s("sdz-ce 0.1.0\n\n护眼：DWM 暖色调色层（不修改系统 gamma 表，零残留）\n休息：20-20-20 提醒\n\n热键：F9 开/关护眼　F8 立即休息　F5 设置");
    let cap = u16s("关于 sdz-ce");
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(cap.as_ptr()), MESSAGEBOX_STYLE(0));
    }
}

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

// ---------------- 设置窗 ----------------

fn slider_pos(h: HWND, id: u32) -> i32 {
    unsafe {
        if let Ok(s) = GetDlgItem(h, id as i32) {
            SendMessageW(s, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32
        } else {
            0
        }
    }
}

fn check_state(h: HWND, id: u32) -> bool {
    unsafe {
        if let Ok(c) = GetDlgItem(h, id as i32) {
            SendMessageW(c, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 as i32 == 1
        } else {
            false
        }
    }
}

fn open_settings(a: &mut App) {
    unsafe {
        let mut cls = CLASS_SET.encode_utf16().collect::<Vec<u16>>();
        cls.push(0);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_null),
            hInstance: hinst(),
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let title = u16s("设置 — sdz-ce");
        let h = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(cls.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            600,
            320,
            None,
            None,
            hinst(),
            None,
        ) {
            Ok(h) => h,
            Err(_) => return,
        };
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);

        // 标签
        let mk_label = |text: &str, x: i32, y: i32| {
            let t = u16s(text);
            let _ = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR("STATIC".encode_utf16().collect::<Vec<u16>>()[..].as_ptr()),
                PCWSTR(t.as_ptr()),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                x,
                y,
                200,
                20,
                h,
                None,
                hinst(),
                None,
            );
        };
        // 滑条（位置 1-based）
        let mk_slider = |id: u32, y: i32, lo: i32, hi: i32, val: i32| {
            let s = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR("msctls_trackbar".encode_utf16().collect::<Vec<u16>>()[..].as_ptr()),
                None,
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                20,
                y,
                250,
                30,
                h,
                menu_id_h(id),
                hinst(),
                None,
            )
            .unwrap_or_default();
            SendMessageW(s, TBM_SETMINPOS, WPARAM(0), LPARAM(lo as isize));
            SendMessageW(s, TBM_SETMAXPOS, WPARAM(0), LPARAM(hi as isize));
            SendMessageW(s, TBM_SETPOS, WPARAM(1), LPARAM(val as isize));
        };
        let mk_chk = |id: u32, text: &str, y: i32, checked: bool| {
            let t = u16s(text);
            let _ = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR("BUTTON".encode_utf16().collect::<Vec<u16>>()[..].as_ptr()),
                PCWSTR(t.as_ptr()),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | 3u32), // BS_AUTOCHECKBOX
                320,
                y,
                250,
                24,
                h,
                menu_id_h(id),
                hinst(),
                None,
            );
            if let Ok(c) = GetDlgItem(h, id as i32) {
                SendMessageW(c, BM_SETCHECK, WPARAM(if checked { 1 } else { 0 }), LPARAM(0));
            }
        };
        let mk_btn = |id: u32, text: &str, x: i32| {
            let t = u16s(text);
            let _ = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR("BUTTON".encode_utf16().collect::<Vec<u16>>()[..].as_ptr()),
                PCWSTR(t.as_ptr()),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                x,
                280,
                90,
                30,
                h,
                menu_id_h(id),
                hinst(),
                None,
            );
        };

        mk_label("护眼强度", 20, 18);
        mk_label("色温", 20, 72);
        mk_label("休息（分）", 20, 126);
        mk_label("专注阈值（分）", 20, 180);
        mk_label("自动开启", 320, 18);
        mk_label("自动关闭", 320, 72);
        mk_label("20-20-20 休息提醒", 320, 130);
        mk_label("按时段自动开关", 320, 156);
        mk_label("开机自启", 320, 182);

        mk_slider(SL_PRESET, 40, 0, 3, a.cfg.preset as i32);
        mk_slider(SL_TEMP, 94, 0, 3, a.cfg.temp as i32);
        mk_slider(SL_REST, 148, 1, 60, a.cfg.rest_min as i32);
        mk_slider(SL_FOCUS, 202, 5, 120, a.cfg.focus_min as i32);
        mk_slider(SL_ONH, 40, 0, 23, (a.cfg.auto_on_hm / 60) as i32);
        mk_slider(SL_ONM, 66, 0, 11, ((a.cfg.auto_on_hm % 60) / 5) as i32);
        mk_slider(SL_OFFH, 94, 0, 23, (a.cfg.auto_off_hm / 60) as i32);
        mk_slider(SL_OFFM, 120, 0, 11, ((a.cfg.auto_off_hm % 60) / 5) as i32);

        mk_chk(CK_2020, "启用休息提醒", 138, a.cfg.auto_break);
        mk_chk(CK_AUTO, "按时段自动开关", 164, a.cfg.auto_time);
        mk_chk(CK_BOOT, "开机自启", 190, a.cfg.autostart);

        mk_btn(BT_OK, "确定", 400);
        mk_btn(BT_CANCEL, "取消", 500);

        // 模态循环（HWND 0 过滤，保证子控件消息正常派发；窗口被销毁即退出）
        let mut msg = MSG::default();
        loop {
            let got = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if got.0 != 0 {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            } else {
                break;
            }
            if msg.message == WM_COMMAND {
                let id = msg.wParam.0 as u32;
                if id == BT_OK {
                    let p = |id: u32| (slider_pos(h, id) - 1).max(0);
                    a.cfg.preset = p(SL_PRESET).min(3) as u8;
                    a.cfg.temp = p(SL_TEMP).min(3) as u8;
                    a.cfg.rest_min = p(SL_REST).clamp(1, 60) as u8;
                    a.cfg.focus_min = p(SL_FOCUS).clamp(5, 120) as u8;
                    a.cfg.auto_break = check_state(h, CK_2020);
                    a.cfg.auto_time = check_state(h, CK_AUTO);
                    a.cfg.autostart = check_state(h, CK_BOOT);
                    a.cfg.auto_on_hm = p(SL_ONH).min(23) as u16 * 60 + p(SL_ONM).min(11) as u16 * 5;
                    a.cfg.auto_off_hm = p(SL_OFFH).min(23) as u16 * 60 + p(SL_OFFM).min(11) as u16 * 5;
                    save_cfg(&a.cfg);
                    apply_tint(a);
                    apply_autostart(a.cfg.autostart);
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &tray_data(a));
                    DestroyWindow(h).ok();
                    break;
                } else if id == BT_CANCEL {
                    DestroyWindow(h).ok();
                    break;
                }
            }
            if IsWindow(h).0 == 0 {
                break;
            }
        }
    }
}

// ---------------- 入口 ----------------

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).ok();

        // 单实例
        let name = u16s("Local\\sdz-ce_mutex");
        if let Ok(_m) = CreateMutexW(None, TRUE, PCWSTR(name.as_ptr())) {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return;
            }
        }

        let mut app = App {
            hidden: HWND::default(),
            tint: HWND::default(),
            hud: HWND::default(),
            cfg: load_cfg(),
            in_break: false,
            hud_left: 0,
        };

        // 隐藏消息窗
        let mut cls = CLASS_HIDDEN.encode_utf16().collect::<Vec<u16>>();
        cls.push(0);
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
            HWND_MESSAGE,
            None,
            hinst(),
            None,
        )
        .unwrap_or_default();

        APP = Box::leak(Box::new(app)) as *mut App;
        let a = &mut *APP;

        create_tint(a);
        create_hud(a);

        let _ = Shell_NotifyIconW(NIM_ADD, &tray_data(a));
        SetTimer(a.hidden, TIMER_IDLE as usize, 10_000, None);

        RegisterHotKey(a.hidden, HK_TOGGLE, MOD_NOREPEAT, 0x78).ok(); // F9
        RegisterHotKey(a.hidden, HK_BREAK, MOD_NOREPEAT, 0x77).ok();  // F8
        RegisterHotKey(a.hidden, HK_SET, MOD_NOREPEAT, 0x74).ok();    // F5

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

        // 清理
        let a = &mut *APP;
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(a));
        DestroyWindow(a.tint).ok();
        DestroyWindow(a.hud).ok();
    }
}
