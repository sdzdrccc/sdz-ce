# sdz-ce

基于 DWM 调色层的轻量护眼工具（Rust 单文件、零运行时依赖）。

## 护眼原理（不用旧 gamma 方案）

不使用 `SetDeviceGammaRamp` 修改系统 gamma 查找表（旧"护眼宝"类工具的崩溃残留、
驱动冲突问题的根源），改为：

- 全屏透明置顶**点击穿透**分层窗（`WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`）
- `SetLayeredWindowAttributes` 叠加 4 档色温 × 3 档强度的暖色蒙版，等效压低蓝光占比
- 颜色由系统 DWM 渲染托管：进程退出或被杀，分层窗随进程自动销毁，**屏幕瞬间恢复，零残留**
- 通过 `SM_XVIRTUALSCREEN / SM_CXVIRTUALSCREEN` 覆盖全部虚拟屏幕（多显示器含负坐标）

## 功能

| 功能 | 说明 |
|---|---|
| 护眼开关 | 托盘左键 / 右键菜单 / `F9` |
| 色温 4 档 × 强度 3 档 | 托盘右键菜单或设置窗滑条 |
| 20-20-20 休息提醒 | `GetLastInputInfo` 空闲检测，达到专注阈值自动弹置顶大字倒计时 + 响铃；`F8` 立即休息 |
| 按时段自动开关 | 如 21:00–次日 7:00 |
| 开机自启 | `HKCU\...\Run`，设置窗可开关 |
| 设置 | `F5` 打开 |
| 单实例 | 命名互斥体 `Local\sdz-ce_mutex` |

配置：`%APPDATA%\sdz-ce\config.ini`（纯文本，可手改）。

## 构建

```powershell
cargo build --release
# 产物：target\release\sdz-ce.exe（约 165 KB，绿色单文件）
```

要求：Windows 10/11，Rust 工具链。无管理员权限要求。

## 热键

| 键 | 作用 |
|---|---|
| `F9` | 开 / 关护眼 |
| `F8` | 立即开始休息 |
| `F5` | 打开设置 |

## 与旧方案（护眼宝）对比

| 维度 | 旧 gamma 方案 | 本工具 DWM 调色层 |
|---|---|---|
| 崩溃/被杀后 | 屏幕颜色残留错乱 | 自动恢复，零残留 |
| 显卡驱动 | 与 NVIDIA/AMD gamma 通道互相覆写 | DWM 标准渲染路径，无冲突 |
| 多屏 | 通常只处理主屏 | 全虚拟屏幕覆盖 |
| 体积 | 4.9 MB（32 位 MFC） | 约 165 KB（64 位 Rust） |
