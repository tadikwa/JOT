use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/jot.ico");
    for name in [
        "RC",
        "WindowsSdkVerBinPath",
        "WindowsSdkDir",
        "WindowsSDKVersion",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    assert_eq!(
        env::var("CARGO_CFG_TARGET_ENV").as_deref(),
        Ok("msvc"),
        "Jot's Windows resource build requires the MSVC Windows SDK"
    );
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("assets/jot.ico");
    let script = out.join("jot.rc");
    let resource = out.join("jot.res");
    // Explorer's Details tab and SmartScreen read the name and version here.
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let numeric = version
        .split(['-', '+'])
        .next()
        .unwrap_or("0.0.0")
        .replace('.', ",");
    fs::write(
        &script,
        format!(
            "1 ICON \"{icon}\"\n\
             1 VERSIONINFO\n\
             FILEVERSION {numeric},0\n\
             PRODUCTVERSION {numeric},0\n\
             FILEOS 0x40004\n\
             FILETYPE 0x1\n\
             BEGIN\n\
             BLOCK \"StringFileInfo\"\n\
             BEGIN\n\
             BLOCK \"040904B0\"\n\
             BEGIN\n\
             VALUE \"CompanyName\", \"tadikwa\"\n\
             VALUE \"FileDescription\", \"Jot - voice dictation\"\n\
             VALUE \"FileVersion\", \"{version}\"\n\
             VALUE \"InternalName\", \"jot\"\n\
             VALUE \"LegalCopyright\", \"Apache-2.0\"\n\
             VALUE \"OriginalFilename\", \"jot.exe\"\n\
             VALUE \"ProductName\", \"Jot\"\n\
             VALUE \"ProductVersion\", \"{version}\"\n\
             END\n\
             END\n\
             BLOCK \"VarFileInfo\"\n\
             BEGIN\n\
             VALUE \"Translation\", 0x409, 1200\n\
             END\n\
             END\n",
            icon = icon.display().to_string().replace('\\', "/")
        ),
    )
    .expect("writing Jot resource script");
    let result = Command::new(resource_compiler())
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&script)
        .output()
        .expect("starting rc.exe; install the Windows SDK or set RC to rc.exe");
    assert!(
        result.status.success(),
        "rc.exe failed: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo:rustc-link-arg={}", resource.display());
}

fn resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("RC") {
        return path.into();
    }
    if let Some(path) = env::var_os("WindowsSdkVerBinPath") {
        let compiler = PathBuf::from(path).join("x64/rc.exe");
        if compiler.is_file() {
            return compiler;
        }
    }
    let sdk = env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                env::var_os("ProgramFiles(x86)").expect("Windows SDK location unavailable"),
            )
            .join("Windows Kits/10")
        });
    if let Some(version) = env::var_os("WindowsSDKVersion") {
        let compiler = sdk.join("bin").join(version).join("x64/rc.exe");
        if compiler.is_file() {
            return compiler;
        }
    }
    let mut compilers: Vec<PathBuf> = fs::read_dir(sdk.join("bin"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64/rc.exe"))
        .filter(|path| path.is_file())
        .collect();
    compilers.sort();
    compilers.pop().unwrap_or_else(|| PathBuf::from("rc.exe"))
}
