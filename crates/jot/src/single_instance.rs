//! One full app at a time: two global hooks would both process the dictation key.
use crate::i18n::{Msg, tf, tr};
use anyhow::{Result, bail};
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Threading::*;
use windows::core::{PCWSTR, w};

pub struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn acquire() -> Result<Instance> {
    unsafe {
        let handle = CreateMutexW(None, false, w!("Local\\com.ammaar.jot.dictation"))?;
        let already_exists = GetLastError() == ERROR_ALREADY_EXISTS;
        let guard = Instance(handle);
        if already_exists {
            bail!("{}", tr(Msg::AlreadyRunning));
        }
        // Older builds do not have the mutex. A portable test must also refuse
        // to install its global hook alongside those builds.
        if jot_core::test_profile::root().is_some() {
            let snapshot = Instance(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
            let mut process = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut next = Process32FirstW(snapshot.0, &mut process);
            loop {
                match next {
                    Ok(()) => {
                        let len = process
                            .szExeFile
                            .iter()
                            .position(|c| *c == 0)
                            .unwrap_or(process.szExeFile.len());
                        let name = String::from_utf16_lossy(&process.szExeFile[..len]);
                        if process.th32ProcessID != std::process::id()
                            && name.eq_ignore_ascii_case("jot.exe")
                        {
                            let other = Instance(OpenProcess(
                                PROCESS_QUERY_LIMITED_INFORMATION,
                                false,
                                process.th32ProcessID,
                            )?);
                            let mut path = [0u16; 32768];
                            let mut len = path.len() as u32;
                            QueryFullProcessImageNameW(
                                other.0,
                                PROCESS_NAME_WIN32,
                                windows::core::PWSTR(path.as_mut_ptr()),
                                &mut len,
                            )?;
                            bail!(
                                "{}",
                                tf(
                                    Msg::PortableAlreadyRunning,
                                    &[
                                        &process.th32ProcessID,
                                        &String::from_utf16_lossy(&path[..len as usize]),
                                    ]
                                )
                            );
                        }
                        next = Process32NextW(snapshot.0, &mut process);
                    }
                    Err(error) if error.code().0 as u32 == (0x80070000 | ERROR_NO_MORE_FILES.0) => {
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(guard)
    }
}

pub fn notify(error: &anyhow::Error) {
    let text: Vec<u16> = format!("{error:#}").encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("Jot"),
            windows::Win32::UI::WindowsAndMessaging::MB_OK,
        );
    }
}
