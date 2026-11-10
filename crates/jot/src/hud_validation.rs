//! Interactive regression test. Uses the production HUD and a separate-process
//! target; never constructs Services or touches credentials, hooks or user data.
use super::*;
use anyhow::{bail, ensure};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::JobObjects::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const COUNT: u32 = WM_APP + 11;
static CLICKS: AtomicUsize = AtomicUsize::new(0);

struct Job(HANDLE);
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct Target {
    child: Child,
    _job: Job,
    hwnd: HWND,
}
impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

unsafe extern "system" fn target_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                CLICKS.fetch_add(1, Ordering::SeqCst);
                LRESULT(0)
            }
            COUNT => LRESULT(CLICKS.load(Ordering::SeqCst) as isize),
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let brush = CreateSolidBrush(COLORREF(0x00785a3c));
                FillRect(dc, &paint.rcPaint, brush);
                let _ = DeleteObject(brush);
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

pub fn target(parent_pid: u32) -> Result<()> {
    // Covers the spawn-to-job-assignment race as well as controller crashes.
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid)? };
    let raw_parent = parent.0 as usize;
    std::thread::spawn(move || {
        let parent = HANDLE(raw_parent as *mut _);
        unsafe {
            let _ = WaitForSingleObject(parent, 80_000);
            let _ = CloseHandle(parent);
        }
        std::process::exit(3);
    });
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: w!("JotHudClickTarget"),
            lpfnWndProc: Some(target_proc),
            ..Default::default()
        };
        ensure!(
            RegisterClassW(&class) != 0,
            "RegisterClassW: {}",
            windows::core::Error::from_win32()
        );
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!("Jot HUD validation target"),
            WS_POPUP,
            0,
            0,
            800,
            240,
            None,
            None,
            HINSTANCE(instance.0),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        println!("{}", hwnd.0 as usize);
        std::io::stdout().flush()?;
        let mut msg = MSG::default();
        loop {
            let result = GetMessageW(&mut msg, None, 0, 0).0;
            if result == 0 {
                break;
            }
            ensure!(result != -1, "GetMessageW failed");
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// Screen pixels, not PrintWindow/IsWindowVisible. The controlled target covers
/// this entire rectangle, so no personal desktop content is saved.
fn capture(rect: RECT, file: &Path) -> Result<Vec<u8>> {
    unsafe {
        DwmFlush()?;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        ensure!(
            width > 0 && height > 0 && width < 5000 && height < 1000,
            "unexpected capture size {width}x{height}"
        );
        let screen = GetDC(None);
        ensure!(!screen.0.is_null(), "GetDC failed");
        let dc = CreateCompatibleDC(screen);
        let mut bitmap = HBITMAP::default();
        let mut old = HGDIOBJ::default();
        let result = (|| -> Result<Vec<u8>> {
            let mut info = BITMAPINFO::default();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: 40,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            old = SelectObject(dc, bitmap);
            BitBlt(
                dc,
                0,
                0,
                width,
                height,
                screen,
                rect.left,
                rect.top,
                SRCCOPY | CAPTUREBLT,
            )?;
            let _ = GdiFlush();
            let pixels =
                std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize)
                    .to_vec();
            let mut header = vec![0u8; 54];
            header[..2].copy_from_slice(b"BM");
            header[2..6].copy_from_slice(&(54u32 + pixels.len() as u32).to_le_bytes());
            header[10..14].copy_from_slice(&54u32.to_le_bytes());
            header[14..18].copy_from_slice(&40u32.to_le_bytes());
            header[18..22].copy_from_slice(&width.to_le_bytes());
            header[22..26].copy_from_slice(&(-height).to_le_bytes());
            header[26..28].copy_from_slice(&1u16.to_le_bytes());
            header[28..30].copy_from_slice(&32u16.to_le_bytes());
            let mut output = std::fs::File::create(file)?;
            output.write_all(&header)?;
            output.write_all(&pixels)?;
            Ok(pixels)
        })();
        if !old.0.is_null() {
            SelectObject(dc, old);
        }
        if !bitmap.0.is_null() {
            let _ = DeleteObject(bitmap);
        }
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        result
    }
}

fn changed_pixels(pixels: &[u8]) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|p| p[..3] != [120, 90, 60])
        .count()
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(data.0 as *mut Vec<MONITORINFO>) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        monitors.push(info);
    }
    BOOL(1)
}

pub fn run(directory: PathBuf) -> Result<()> {
    let _instance = crate::single_instance::acquire()?;
    std::fs::create_dir_all(&directory)?;
    ensure!(
        !directory.join("result.json").exists(),
        "use a fresh report directory"
    );
    let mut monitors = Vec::<MONITORINFO>::new();
    ensure!(
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(collect_monitor),
                LPARAM(&mut monitors as *mut _ as isize),
            )
        }
        .as_bool(),
        "EnumDisplayMonitors failed"
    );
    ensure!(!monitors.is_empty(), "no display available");
    let job = unsafe {
        let job = Job(CreateJobObjectW(None, None)?);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        )?;
        job
    };
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--hud-click-target")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    if let Err(error) = unsafe { AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())) } {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    // Last-resort bound even if the GPUI loop or target message loop hangs.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(90));
        std::process::exit(3);
    });
    let mut line = String::new();
    let read = BufReader::new(child.stdout.take().unwrap()).read_line(&mut line);
    let hwnd = line.trim().parse::<usize>().map(|v| HWND(v as *mut _));
    if read.is_err() || hwnd.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        bail!("target did not return a HWND: {line:?}");
    }
    let target = Target {
        child,
        _job: job,
        hwnd: hwnd.unwrap(),
    };
    let mut owner = 0;
    unsafe {
        GetWindowThreadProcessId(target.hwnd, Some(&mut owner));
    }
    ensure!(owner == target.child.id(), "target HWND/PID mismatch");
    std::fs::write(
        directory.join("processes.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "hud_pid":std::process::id(),"target_pid":target.child.id(),"target_hwnd":target.hwnd.0 as usize,
            "exe":std::env::current_exe()?,"monitor_count":monitors.len()
        }))?,
    )?;
    let error = Arc::new(parking_lot::Mutex::new(None));
    let app_error = error.clone();
    let show_idle = !std::env::args().any(|arg| arg == "--initially-hidden");
    application().run(move |cx: &mut App| {
        theme::load_fonts(cx);
        cx.set_quit_mode(gpui::QuitMode::Explicit);
        let hud = match open_hud(cx, show_idle) {
            Ok(h) => h,
            Err(e) => { *app_error.lock() = Some(e.to_string()); cx.quit(); return; }
        };
        cx.spawn(async move |cx| {
            let result: Result<()> = async {
                let mut rows = Vec::new();
                for (monitor_index, monitor) in monitors.iter().enumerate() {
                    let work = monitor.rcWork;
                    unsafe { SetWindowPos(target.hwnd, HWND_NOTOPMOST,
                        work.left + (work.right - work.left - 900) / 2, work.bottom - 240, 900, 220, SWP_NOACTIVATE)?; }
                    hud.update(cx, |_, window, _| window_shell::place_on_active_display(window, pill::HUD_BOTTOM_MARGIN))?;
                for cycle in 0..2 {
                    for (name, state) in [
                        ("initial", if show_idle { PillState::IdleDot } else { PillState::Hidden }),
                        ("control", PillState::Hidden),
("idle", PillState::IdleDot),
                        ("hidden", PillState::Hidden),
                        ("warming", PillState::Listening { locked: false }),
                        ("recording", PillState::Listening { locked: false }),
                        ("locked", PillState::Listening { locked: true }),
                        ("processing", PillState::Processing),
                        ("success", PillState::Success { words: 7 }),
                        ("notice", PillState::Notice("Copied — press Ctrl+V to paste".into())),
                        ("error", PillState::Error("Validation error".into())),
                        ("hidden-end", PillState::Hidden),
                    ] {
                        let hwnd = hud.update(cx, |view, window, cx| {
                            view.state = state.clone(); view.level = 0.65;
                            view.recording_since = Some(Instant::now());
                            cx.notify();
                            window_shell::hwnd_of(window).unwrap()
                        })?;
                        let hwnd = HWND(hwnd as *mut _);
                        unsafe { let _ = ShowWindow(hwnd, if name == "control" { SW_HIDE } else { SW_SHOWNOACTIVATE }); }
                        let mut rect = RECT::default();
                        unsafe {
                            GetWindowRect(hwnd, &mut rect)?;
                            SetWindowPos(target.hwnd, HWND_NOTOPMOST, rect.left - 30, rect.top - 30,
                                rect.right - rect.left + 60, rect.bottom - rect.top + 60, SWP_NOACTIVATE)?;
                        }
                        cx.background_executor().timer(Duration::from_millis(450)).await;
                        let first = capture(rect, &directory.join(format!("{monitor_index}-{cycle}-{name}.bmp")))?;
                        let visible_pixels = changed_pixels(&first);
                        let animated = matches!(state, PillState::Listening { .. } | PillState::Processing);
                        let mut animation_ok = true;
                        if animated {
                            cx.background_executor().timer(Duration::from_millis(240)).await;
                            let second = capture(rect, &directory.join(format!("{monitor_index}-{cycle}-{name}-frame2.bmp")))?;
                            animation_ok = first.chunks_exact(4).zip(second.chunks_exact(4)).any(|(a,b)| a[..3] != b[..3]);
                        }
                        let mut client = RECT::default();
                        unsafe { GetClientRect(hwnd, &mut client)?; }
                        let dpi = unsafe { GetDpiForWindow(hwnd) };
                        let size_ok = (client.right as f32 - pill::HUD_SIZE.0 * dpi as f32 / 96.0).abs() <= 1.0
                            && (client.bottom as f32 - pill::HUD_SIZE.1 * dpi as f32 / 96.0).abs() <= 1.0;
                        let placement = window_shell::bottom_center(work.left, work.top, work.right, work.bottom,
                            rect.right - rect.left, rect.bottom - rect.top, (pill::HUD_BOTTOM_MARGIN * dpi as f32 / 96.0).round() as i32);
                        let placement_ok = rect.left == placement.x && rect.top == placement.y;
                        let focus_before = unsafe { GetForegroundWindow() };
                        let count_before = unsafe { SendMessageW(target.hwnd, COUNT, WPARAM(0), LPARAM(0)).0 };
                        // Select a pixel actually drawn at the centre of the pill,
                        // rather than guessing its location from the window margin.
                        let width = (rect.right - rect.left) as usize;
                        let height = (rect.bottom - rect.top) as usize;
                        let x = width / 2;
                        let occupied: Vec<_> = (0..height).filter(|y| {
                            let index = (y * width + x) * 4;
                            first[index..index + 3] != [120,90,60]
                        }).collect();
                        let pill_y = occupied.get(occupied.len() / 2).copied().unwrap_or(height / 2);
                        let pill_index = (pill_y * width + x) * 4;
                        let pill_sample = &first[pill_index..pill_index + 3];
                        let pill_hit_drawn_pixel = pill_sample != [120,90,60];
                        for (x, y) in [(rect.left + x as i32, rect.top + pill_y as i32), (rect.left + 4, rect.top + 4)] {
                            unsafe {
                                SetCursorPos(x, y)?;
                                let inputs = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
                                    r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } }
                                });
                                ensure!(SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) == 2, "SendInput failed");
                            }
                            cx.background_executor().timer(Duration::from_millis(90)).await;
                        }
                        let clicks = unsafe { SendMessageW(target.hwnd, COUNT, WPARAM(0), LPARAM(0)).0 - count_before };
                        let focus_after = unsafe { GetForegroundWindow() };
                        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 };
                        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 };
                        let mut pid = 0;
                        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
                        let mut class = [0u16; 256];
                        let class_len = unsafe { GetClassNameW(hwnd, &mut class) };
                        let parent = unsafe { GetParent(hwnd) }.unwrap_or_default();
                        let owner = unsafe { GetWindow(hwnd, GW_OWNER) }.unwrap_or_default();
                        let hidden = matches!(state, PillState::Hidden);
                        let backdrop_ok = first.chunks_exact(4).take((rect.right - rect.left) as usize).all(|p| p[..3] == [120,90,60]);
                        let rendered = backdrop_ok && if hidden { visible_pixels == 0 } else { visible_pixels >= 50 && visible_pixels < first.len() / 4 * 7 / 10 };
                        let focus_ok = focus_before == target.hwnd && focus_after == target.hwnd;
                        let rendered = rendered && (hidden || pill_hit_drawn_pixel);
                        let row = serde_json::json!({"monitor":monitor_index,"work_area":[work.left,work.top,work.right,work.bottom],"dpi":dpi,"size_ok":size_ok,"placement_ok":placement_ok,"animation_ok":animation_ok,"cycle":cycle,"state":name,"hwnd":hwnd.0 as usize,"pid":pid,"thread":thread,
                            "class":String::from_utf16_lossy(&class[..class_len as usize]),"parent":parent.0 as usize,"owner":owner.0 as usize,"target_pid":target.child.id(),"style":format!("0x{style:08X}"),"ex_style":format!("0x{ex_style:08X}"),
                            "rect":[rect.left,rect.top,rect.right,rect.bottom],"visible_pixels":visible_pixels,
                            "rendered":rendered,"backdrop_ok":backdrop_ok,"pill_click":[rect.left+x as i32,rect.top+pill_y as i32],"pill_sample_bgr":pill_sample,"pill_hit_drawn_pixel":pill_hit_drawn_pixel,"received_clicks":clicks,"expected_clicks":2,"focus_ok":focus_ok,
                            "topmost":ex_style & WS_EX_TOPMOST.0 != 0});
                        rows.push(row);
                        std::fs::write(directory.join("result.json"), serde_json::to_vec_pretty(&rows)?)?;
                    }
                }
                }
                ensure!(rows.iter().all(|r| r["size_ok"] == true && r["placement_ok"] == true && r["animation_ok"] == true && r["rendered"] == true && r["received_clicks"] == 2 && r["focus_ok"] == true && r["topmost"] == true), "HUD regression: inspect result.json");
                Ok(())
            }.await;
            if let Err(e) = result { *app_error.lock() = Some(format!("{e:#}")); }
            drop(target);
            let _ = cx.update(|cx| cx.quit());
        }).detach();
    });
    if let Some(error) = error.lock().take() {
        bail!(error);
    }
    Ok(())
}

pub fn profile_check(file: PathBuf) -> Result<()> {
    ensure!(
        jot_core::test_profile::root().is_some(),
        "profile check requires the portable marker or JOT_TEST_PROFILE=1"
    );
    ensure!(!file.exists(), "report already exists");
    let instance = crate::single_instance::acquire();
    let result = serde_json::json!({
        "pid":std::process::id(),"exe":std::env::current_exe()?,
        "root":jot_core::file_layout::app_support_root(),
        "settings":jot_core::file_layout::settings_json(),
        "dictionary":jot_core::file_layout::dictionary_json(),
        "history":jot_core::file_layout::history_sqlite(),
        "recordings":jot_core::file_layout::recordings_root(),
        "autostart_enabled":crate::autostart::is_enabled(),
        "autostart_stale":crate::autostart::is_stale(),
        "autostart_write_allowed":crate::autostart::set_enabled(true),
        "instance_ok":instance.is_ok(),"instance_error":instance.err().map(|e| format!("{e:#}"))
    });
    std::fs::write(file, serde_json::to_vec_pretty(&result)?)?;
    Ok(())
}
