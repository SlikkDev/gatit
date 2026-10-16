use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=GATIT_VERSION");
    // Linux cross-target clippy does not link executables or have the Windows SDK.
    if !env::var("HOST").unwrap_or_default().contains("windows") {
        return;
    }
    let version = env::var("GATIT_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let parts: Vec<u16> = version
        .split('.')
        .map(|part| part.parse().expect("numeric release version"))
        .collect();
    assert_eq!(parts.len(), 3, "major.minor.patch required");
    let numeric = format!("{},{},{},0", parts[0], parts[1], parts[2]);
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rc = out.join("gatit-version.rc");
    let res = out.join("gatit-version.res");
    fs::write(
        &rc,
        format!(
            r#"1 VERSIONINFO
 FILEVERSION {numeric}
 PRODUCTVERSION {numeric}
 FILEOS 0x40004
 FILETYPE 0x1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "ProductName", "Gatit\0"
   VALUE "ProductVersion", "{version}\0"
   VALUE "FileDescription", "Gatit recorder\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "OriginalFilename", "gatit-recorder.exe\0"
   VALUE "LegalCopyright", "Copyright (c) 2026 Yarin Barry\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#
        ),
    )
    .unwrap();
    let sdk = PathBuf::from(env::var_os("ProgramFiles(x86)").unwrap()).join("Windows Kits/10/bin");
    let mut candidates: Vec<_> = fs::read_dir(sdk)
        .expect("Windows SDK is required")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64/rc.exe"))
        .filter(|path| path.is_file())
        .collect();
    candidates.sort();
    let compiler = candidates.last().expect("Windows SDK rc.exe is required");
    let status = Command::new(compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc)
        .status()
        .expect("Windows SDK rc.exe is required");
    assert!(status.success(), "resource compilation failed");
    println!("cargo:rustc-link-arg={}", res.display());
}
