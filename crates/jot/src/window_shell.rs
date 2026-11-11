// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The two things the HUD needs from Win32 that GPUI does not expose.
//!
//! `WindowKind::PopUp` already gives a borderless, topmost, tool window. What it
//! does not give is a window that refuses focus and passes clicks through, and
//! GPUI has no cross-platform way to move a window after it is open. Both are
//! load-bearing here: a HUD that takes focus breaks the very insertion it is
//! announcing, and one that eats clicks makes the bottom of every screen dead.

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Where the pill should sit, in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
}

/// Bottom-centre of the work area, given the monitor rectangle and window size.
///
/// Pure so the arithmetic is testable: an off-by-one here parks the pill under
/// the taskbar or half off the screen.
pub fn bottom_center(
    work_left: i32,
    work_top: i32,
    work_right: i32,
    work_bottom: i32,
    width: i32,
    height: i32,
    bottom_margin: i32,
) -> Placement {
    let center = work_left + (work_right - work_left) / 2;
    Placement {
        x: center - width / 2,
        // Clamped so a monitor shorter than the HUD still shows its top edge.
        y: (work_bottom - height - bottom_margin).max(work_top),
    }
}

/// Is the Windows high-contrast setting on?
///
/// GPUI reports reduced motion but not this, and a theme that ignores it is
/// unreadable for the people who turned it on.
pub fn high_contrast() -> bool {
    platform::high_contrast()
}

pub fn hwnd_of(window: &Window) -> Option<isize> {
    // Fully qualified: GPUI's own `Window::window_handle` returns its
    // `AnyWindowHandle`, which is a different thing entirely.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Marks a window as a passive overlay — never focused, never clicked — and
/// places it inside the work area of the active display.
///
/// Placement is part of this rather than a later step because the taskbar is
/// topmost as well: a pill positioned against the screen edge is simply drawn
/// behind it, which looks exactly like a pill that never appeared.
pub fn make_overlay(window: &Window, bottom_margin: f32) -> anyhow::Result<()> {
    let hwnd = hwnd_of(window).ok_or_else(|| anyhow::anyhow!("HUD has no Win32 handle"))?;
    platform::make_overlay(hwnd)?;
    platform::place_on_active_display(hwnd, bottom_margin)
}

/// Moves the HUD to the bottom centre of the display holding the foreground
/// window — where the text is about to land.
pub fn place_on_active_display(window: &Window, bottom_margin: f32) {
    if let Some(hwnd) = hwnd_of(window)
        && let Err(error) = platform::place_on_active_display(hwnd, bottom_margin)
    {
        tracing::error!(%error, "could not position the HUD");
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::bottom_center;
    use windows::Win32::Foundation::{
        COLORREF, ERROR_SUCCESS, GetLastError, HWND, RECT, SetLastError,
    };
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST,
        IsWindowVisible, LWA_ALPHA, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE,
        SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_EX_WINDOWEDGE,
    };

    /// `DWMWA_COLOR_NONE` — documented, but not exported by the `windows` crate.
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;

    pub fn high_contrast() -> bool {
        use windows::Win32::UI::Accessibility::HIGHCONTRASTW;
        use windows::Win32::UI::WindowsAndMessaging::{
            SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
        };
        let mut info = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        let queried = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                info.cbSize,
                Some(&mut info as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        // HCF_HIGHCONTRASTON
        queried.is_ok() && info.dwFlags.0 & 0x0000_0001 != 0
    }

    pub fn make_overlay(hwnd: isize) -> anyhow::Result<()> {
        let hwnd = HWND(hwnd as *mut _);
        unsafe {
            SetLastError(ERROR_SUCCESS);
            let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            anyhow::ensure!(
                current != 0 || GetLastError() == ERROR_SUCCESS,
                "GetWindowLongPtrW: {}",
                windows::core::Error::from_win32()
            );
            // WS_EX_TRANSPARENT alone only orders painting. Layered + transparent
            // provides cross-process input transparency; initialize or it stays invisible.
            // GPUI uses DirectComposition, never UpdateLayeredWindow (which SLA disables).
            let wanted = (current & !WS_EX_WINDOWEDGE.0)
                | WS_EX_LAYERED.0
                | WS_EX_TRANSPARENT.0
                | WS_EX_NOACTIVATE.0
                | WS_EX_TOOLWINDOW.0;
            SetLastError(ERROR_SUCCESS);
            let previous = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted as isize);
            anyhow::ensure!(
                previous != 0 || GetLastError() == ERROR_SUCCESS,
                "SetWindowLongPtrW: {}",
                windows::core::Error::from_win32()
            );
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

            // Windows 11 draws a rounded 1px border around EVERY top-level
            // window, including a borderless transparent one. On the HUD that
            // reads as a grey box around the pill.
            //
            // Three attributes are needed, not one: the corner preference stops
            // the rounding, the border colour removes the left, right and bottom
            // edges, and only disabling non-client rendering removes the top
            // one — DWM draws that edge as caption chrome, which the border
            // colour does not cover.
            let policy = DWMNCRP_DISABLED;
            if let Err(error) = DwmSetWindowAttribute(
                hwnd,
                DWMWA_NCRENDERING_POLICY,
                &policy as *const _ as *const _,
                std::mem::size_of_val(&policy) as u32,
            ) {
                tracing::debug!(%error, "HUD non-client rendering attribute unavailable");
            }
            let corner = DWMWCP_DONOTROUND;
            if let Err(error) = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const _,
                std::mem::size_of_val(&corner) as u32,
            ) {
                tracing::debug!(%error, "HUD corner attribute unavailable");
            }
            let none = DWMWA_COLOR_NONE;
            if let Err(error) = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                &none as *const _ as *const _,
                std::mem::size_of_val(&none) as u32,
            ) {
                tracing::debug!(%error, "HUD border attribute unavailable");
            }
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE_NOSIZE_NOACTIVATE)?;
            // Explicitly, and without activating. A `WS_EX_NOACTIVATE` popup is
            // left hidden by the normal show path, which makes the pill exist
            // and never appear — the worst possible failure for a HUD.
            // ShowWindow returns the *previous* visibility, not success/failure.
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            anyhow::ensure!(IsWindowVisible(hwnd).as_bool(), "HUD remained hidden");
            let applied = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            anyhow::ensure!(
                applied & wanted == wanted && applied & WS_EX_TOPMOST.0 != 0,
                "HUD styles not applied: {applied:#010x}"
            );
            tracing::info!(
                hwnd = hwnd.0 as usize,
                ex_style = applied,
                "passive layered HUD initialized"
            );
        }
        Ok(())
    }

    const SWP_NOMOVE_NOSIZE_NOACTIVATE:
        windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS =
        windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS(
            0x0002 /* SWP_NOMOVE */ | SWP_NOSIZE.0 | SWP_NOACTIVATE.0,
        );

    pub fn place_on_active_display(hwnd: isize, bottom_margin: f32) -> anyhow::Result<()> {
        let hwnd = HWND(hwnd as *mut _);
        unsafe {
            let target = GetForegroundWindow();
            let monitor = MonitorFromWindow(
                if target.0.is_null() { hwnd } else { target },
                MONITOR_DEFAULTTONEAREST,
            );
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            anyhow::ensure!(
                GetMonitorInfoW(monitor, &mut info).as_bool(),
                "GetMonitorInfoW: {}",
                windows::core::Error::from_win32()
            );
            // Moving between DPI domains synchronously invokes GPUI's WM_DPICHANGED.
            // Re-read its resulting physical size and DPI before the final centring.
            for _ in 0..2 {
                let mut rect = RECT::default();
                GetWindowRect(hwnd, &mut rect)?;
                let dpi = GetDpiForWindow(hwnd);
                anyhow::ensure!(dpi != 0, "GetDpiForWindow failed");
                let work = info.rcWork;
                let placement = bottom_center(
                    work.left,
                    work.top,
                    work.right,
                    work.bottom,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    (bottom_margin * dpi as f32 / 96.0).round() as i32,
                );
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    placement.x,
                    placement.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    pub fn high_contrast() -> bool {
        false
    }
    pub fn make_overlay(_hwnd: isize) -> anyhow::Result<()> {
        Ok(())
    }
    pub fn place_on_active_display(_hwnd: isize, _margin: f32) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pill_centres_horizontally_and_sits_above_the_taskbar() {
        // A 1920x1080 display with a 40px taskbar.
        let placement = bottom_center(0, 0, 1920, 1040, 600, 96, 16);
        assert_eq!(placement.x, 660);
        assert_eq!(placement.y, 1040 - 96 - 16);
    }

    #[test]
    fn a_secondary_display_to_the_left_gets_negative_coordinates() {
        let placement = bottom_center(-1920, 0, 0, 1080, 600, 96, 16);
        assert_eq!(placement.x, -1260);
    }

    #[test]
    fn a_display_shorter_than_the_hud_still_shows_its_top_edge() {
        let placement = bottom_center(0, 0, 800, 60, 600, 96, 16);
        assert_eq!(placement.y, 0);
    }
}
