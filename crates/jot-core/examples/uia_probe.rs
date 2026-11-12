//! Feasibility probe for learning from corrections made after an insertion.
//!
//! cargo run -p jot-core --example uia_probe -- <window title part> <scenario>
//!
//! Scenarios: fix (Loustream → Loostream inside the segment), outside (an
//! edit before the segment only), none (no edit), long (≈700 chars, several
//! lines, then the same fix), timeout (fix, then let the 30 s window expire).
//! Prints the field's UIA identity, the insertion outcome, how long the
//! insertion receipt took, read latency, CPU used and what would be learned.
//! Never prints the field's text. Synthetic keys stand in for the user, so the
//! probe also bumps the key counter the real keyboard hook bumps.

#[cfg(not(target_os = "windows"))]
fn main() {}

#[cfg(target_os = "windows")]
fn main() {
    win::main();
}

#[cfg(target_os = "windows")]
mod win {
    use jot_core::app_learning::{self, Watch};
    use jot_core::insertion::{InsertionCoordinator, TextInserting};
    use jot_core::transcription::DictationContext;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{BOOL, FILETIME, HWND, LPARAM};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::System::Threading::{
        AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, GetProcessTimes,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ControlTypePropertyId,
        UIA_DocumentControlTypeId, UIA_IsTextPatternAvailablePropertyId, UIA_NamePropertyId,
        UIA_TextPattern2Id, UIA_TextPatternId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_END, VK_HOME, VK_LEFT,
        VK_RETURN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowTextW,
        GetWindowThreadProcessId, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    use windows::core::VARIANT;

    const SHORT: &str = "Je vais ouvrir Loustream.";

    fn long_text() -> String {
        let line = "Le réseau Kubernetes redémarre, puis PowerShell relance le service.";
        let mut text = String::new();
        for _ in 0..9 {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(SHORT);
        text
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let (title, scenario) = (args[0].as_str(), args.get(1).map_or("fix", String::as_str));
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let hwnd = find_window(title).expect("no visible window with that title");
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let clipboard = jot_core::win32::snapshot_clipboard();
        if !foreground(hwnd) {
            println!(
                "foreground refused (target {:?}, front {:?}) — no key sent",
                hwnd.0,
                unsafe { GetForegroundWindow() }.0
            );
            return;
        }
        // Keys go only to a text field of the target: never to a menu item.
        if !focus_text(hwnd, pid, args.get(2).map(String::as_str)) {
            describe();
            println!("no UIA text field focused in the target — no key sent (clean refusal)");
            return;
        }
        // Caret at the document end, on a fresh line, like a user about to dictate.
        chord(VK_CONTROL, VK_END);
        key(VK_RETURN);
        app_learning::note_key_down();
        std::thread::sleep(Duration::from_millis(300));
        describe();

        let text = if scenario == "long" {
            long_text()
        } else {
            SHORT.to_string()
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        jot_core::runtime::install(runtime.handle().clone());
        let context = DictationContext {
            target_pid: Some(pid),
            ..Default::default()
        };
        let inserted_at = Instant::now();
        // JOT_PROBE_SLOW: same typing, one character at a time (diagnostic).
        let outcome = if std::env::var_os("JOT_PROBE_SLOW").is_some() {
            for c in text.chars() {
                jot_core::win32::type_text(&c.to_string());
                std::thread::sleep(Duration::from_millis(15));
            }
            jot_core::insertion::InsertionOutcome::Inserted
        } else {
            runtime.block_on(InsertionCoordinator::new().insert(&text, &context))
        };
        println!("insertion: {outcome:?} ({} chars)", text.chars().count());

        // The receipt: how long until the text provably sits before the caret.
        let receipt = loop {
            let read = app_learning::probe_focused(Some(pid), text.chars().count() + 48, 48);
            if read.is_some_and(|r| {
                app_learning::anchor(&text, &r.before, &r.after, r.at_start, r.at_end).is_some()
            }) {
                break Some(inserted_at.elapsed());
            }
            if inserted_at.elapsed() > Duration::from_secs(2) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        println!("receipt: {receipt:?}");
        // Fictitious documents only: shows what the read saw near the caret.
        if std::env::var_os("JOT_PROBE_DEBUG").is_some() {
            match app_learning::probe_focused(Some(pid), text.chars().count() + 48, 48) {
                Some(r) => println!(
                    "debug: before {} chars ends {:?} | after {:?} | start {} end {}",
                    r.before.chars().count(),
                    r.before
                        .chars()
                        .rev()
                        .take(60)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>(),
                    r.after.chars().take(60).collect::<String>(),
                    r.at_start,
                    r.at_end
                ),
                None => println!("debug: no read"),
            }
        }
        let reads: Vec<u128> = (0..10)
            .map(|_| {
                let t = Instant::now();
                let _ = app_learning::probe_focused(Some(pid), 400, 400);
                t.elapsed().as_micros()
            })
            .collect();
        println!("read 800 chars around the caret: {:?} µs", reads);

        let (tx, rx) = mpsc::channel();
        app_learning::watch(Watch {
            dictation_id: uuid::Uuid::new_v4(),
            inserted: text.clone(),
            target_pid: Some(pid),
            sink: Box::new(move |c| {
                let _ = tx.send(c);
            }),
        });
        let cpu_before = cpu_ms();
        let watch_started = Instant::now();
        std::thread::sleep(Duration::from_millis(800));
        match scenario {
            "fix" | "long" | "timeout" | "e2e" => {
                // Caret after "Lou" in "Loustream.", replace the "u" by an "o".
                for _ in 0..7 {
                    key(VK_LEFT);
                }
                key(VK_BACK);
                type_char('o');
                (0..9).for_each(|_| app_learning::note_key_down());
            }
            "outside" => {
                chord(VK_CONTROL, VK_HOME);
                for c in "Note ".chars() {
                    type_char(c);
                }
                (0..6).for_each(|_| app_learning::note_key_down());
            }
            _ => {}
        }
        if scenario != "timeout" {
            std::thread::sleep(Duration::from_millis(1500));
            // What a new dictation does.
            app_learning::stop(true);
        }
        let limit = Duration::from_secs(if scenario == "timeout" { 35 } else { 3 });
        let learned = rx.recv_timeout(limit).ok();
        let wall = watch_started.elapsed();
        println!(
            "watch: {:.1} s wall, {} ms CPU for the whole probe process",
            wall.as_secs_f64(),
            cpu_ms() - cpu_before
        );
        match &learned {
            Some(c) => {
                let suggestions = jot_core::dictionary::extract_suggestions(c);
                let pairs: Vec<_> = suggestions
                    .iter()
                    .map(|s| format!("{} → {}", s.wrong, s.term))
                    .collect();
                println!("learned: correction seen, suggestions {pairs:?}");
            }
            None => println!("learned: nothing"),
        }
        if scenario == "e2e" {
            e2e(learned, &context, &runtime);
        }
        // The clipboard fallback must not cost the user their clipboard.
        if !clipboard.is_empty() {
            jot_core::win32::restore_clipboard(clipboard);
        }
    }

    fn describe() {
        unsafe {
            let uia: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
            let Ok(element) = uia.GetFocusedElement() else {
                println!("focused: none");
                return;
            };
            println!(
                "focused: control type {} | framework {:?} | class {:?} | password {} | TextPattern {} | TextPattern2 {}",
                element
                    .CurrentControlType()
                    .map(|c| c.0)
                    .unwrap_or_default(),
                element
                    .CurrentFrameworkId()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                element
                    .CurrentClassName()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                element
                    .CurrentIsPassword()
                    .map(|b| b.as_bool())
                    .unwrap_or_default(),
                element.GetCurrentPattern(UIA_TextPatternId).is_ok(),
                element.GetCurrentPattern(UIA_TextPattern2Id).is_ok(),
            );
        }
    }

    fn cpu_ms() -> u64 {
        let (mut a, mut b, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe {
            let _ = GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut kernel, &mut user);
        }
        let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        (ticks(kernel) + ticks(user)) / 10_000
    }

    fn find_window(part: &str) -> Option<HWND> {
        struct Search(String, Option<HWND>);
        unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
            let search = unsafe { &mut *(data.0 as *mut Search) };
            let mut buffer = [0u16; 512];
            let len = unsafe { GetWindowTextW(hwnd, &mut buffer) } as usize;
            let title = String::from_utf16_lossy(&buffer[..len]);
            if unsafe { IsWindowVisible(hwnd) }.as_bool() && title.contains(&search.0) {
                search.1 = Some(hwnd);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut search = Search(part.to_string(), None);
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut search as *mut Search as isize));
        }
        search.1
    }

    /// From the correction to the next dictation, through the same store calls
    /// the app makes (services.rs sink, Dictionary "Accepter", local pipeline).
    fn e2e(
        learned: Option<jot_core::history::Correction>,
        context: &DictationContext,
        runtime: &tokio::runtime::Runtime,
    ) {
        use jot_core::dictionary::{DictionaryStore, SuggestionStatus};
        let path = std::path::PathBuf::from(
            std::env::var_os("JOT_PROBE_DICT")
                .expect("JOT_PROBE_DICT: a throwaway dictionary.json"),
        );
        let store = DictionaryStore::open(path.clone());
        let Some(correction) = learned else {
            println!("e2e: no correction to learn");
            return;
        };
        let added = store.learn_from_app_correction(&correction).unwrap();
        let pending: Vec<_> = store
            .suggestions()
            .into_iter()
            .filter(|s| s.status == SuggestionStatus::Pending)
            .collect();
        for s in &pending {
            println!(
                "e2e: pending {} → {} (term only {}, sources {:?})",
                s.wrong,
                s.term,
                s.term_only,
                s.sources.iter().map(|s| s.origin).collect::<Vec<_>>()
            );
        }
        let next = "Ouvre Loustream maintenant.";
        println!(
            "e2e: {added} suggestion(s); before review, next dictation {:?} → {:?}",
            next,
            jot_core::replacement::apply(&store.replacement_rules(), next)
        );
        for s in &pending {
            store.accept_suggestion(s.id).unwrap();
        }
        let corrected = jot_core::replacement::apply(&store.replacement_rules(), next);
        println!(
            "e2e: after Accepter, deterministic rule {:?} → {:?}; recognition vocabulary {:?}",
            next,
            corrected,
            store.vocabulary()
        );
        let outcome = runtime.block_on(InsertionCoordinator::new().insert(&corrected, context));
        std::thread::sleep(Duration::from_millis(300));
        let landed = app_learning::probe_focused(context.target_pid, 64, 0)
            .is_some_and(|r| r.before.ends_with(&corrected));
        println!(
            "e2e: next dictation inserted {outcome:?}, reads back corrected in the field: {landed}"
        );
        let json = std::fs::read_to_string(&path).unwrap();
        println!(
            "e2e: dictionary.json {} bytes, origin application {}, holds field context {}",
            json.len(),
            json.contains("\"origin\": \"application\""),
            json.contains("Je vais")
        );
    }

    /// Focuses the field (by UIA name, else the first document with a text
    /// pattern) and checks the focused element really is a text field of `pid`.
    fn focus_text(hwnd: HWND, pid: u32, name: Option<&str>) -> bool {
        unsafe {
            let Ok(uia) =
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            else {
                return false;
            };
            let found = (|| {
                let root = uia.ElementFromHandle(hwnd)?;
                let text = uia.CreatePropertyCondition(
                    UIA_IsTextPatternAvailablePropertyId,
                    &VARIANT::from(true),
                )?;
                let which = match name {
                    Some(name) => {
                        uia.CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(name))?
                    }
                    None => uia.CreatePropertyCondition(
                        UIA_ControlTypePropertyId,
                        &VARIANT::from(UIA_DocumentControlTypeId.0),
                    )?,
                };
                let both = uia.CreateAndCondition(&text, &which)?;
                let field = root.FindFirst(TreeScope_Descendants, &both)?;
                field.SetFocus()
            })();
            if found.is_err() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(300));
            uia.GetFocusedElement().is_ok_and(|f| {
                f.GetCurrentPattern(UIA_TextPatternId).is_ok()
                    && f.CurrentProcessId().is_ok_and(|p| p as u32 == pid)
            })
        }
    }

    /// True only once `hwnd` is really in front: no key is ever sent elsewhere.
    /// No Alt trick: a lone Alt opens the target's menu bar.
    fn foreground(hwnd: HWND) -> bool {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
        std::thread::sleep(Duration::from_millis(300));
        if unsafe { GetForegroundWindow() } != hwnd {
            // Foreground lock: borrow the input queue of the window in front.
            unsafe {
                let front = GetWindowThreadProcessId(GetForegroundWindow(), None);
                let me = GetCurrentThreadId();
                let _ = AttachThreadInput(me, front, true);
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
                let _ = AttachThreadInput(me, front, false);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        if unsafe { GetForegroundWindow() } != hwnd {
            // Last resort: UI Automation focus, which crosses the lock.
            unsafe {
                if let Ok(uia) =
                    CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    && let Ok(element) = uia.ElementFromHandle(hwnd)
                {
                    let _ = element.SetFocus();
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        unsafe { GetForegroundWindow() == hwnd }
    }

    fn send(inputs: &[INPUT]) {
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        std::thread::sleep(Duration::from_millis(40));
    }

    fn input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn key(vk: VIRTUAL_KEY) {
        send(&[
            input(vk, 0, KEYBD_EVENT_FLAGS(0)),
            input(vk, 0, KEYEVENTF_KEYUP),
        ]);
    }

    fn chord(modifier: VIRTUAL_KEY, vk: VIRTUAL_KEY) {
        send(&[
            input(modifier, 0, KEYBD_EVENT_FLAGS(0)),
            input(vk, 0, KEYBD_EVENT_FLAGS(0)),
            input(vk, 0, KEYEVENTF_KEYUP),
            input(modifier, 0, KEYEVENTF_KEYUP),
        ]);
    }

    fn type_char(c: char) {
        let mut units = [0u16; 2];
        for unit in c.encode_utf16(&mut units) {
            send(&[
                input(VIRTUAL_KEY(0), *unit, KEYEVENTF_UNICODE),
                input(VIRTUAL_KEY(0), *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
            ]);
        }
    }
}
