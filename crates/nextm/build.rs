//! Risorse dell'eseguibile (manifest e VERSIONINFO) e opzioni del linker.

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v2">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Le DLL importate dall'exe si cercano solo in System32 (difesa dal DLL planting).
    println!("cargo::rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    let mut res = winresource::WindowsResource::new();
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let icon_path = std::path::Path::new(&manifest_dir).join("../../logo/ICO/hal_ecg_app.ico");
    if icon_path.exists() {
        println!("cargo::rerun-if-changed={}", icon_path.display());
        if let Some(s) = icon_path.to_str() {
            res.set_icon(s);
        }
    }

    res.set("FileDescription", "nextm")
        .set("ProductName", "nextm")
        .set("LegalCopyright", "MIT OR Apache-2.0")
        .set_manifest(MANIFEST);
    if let Err(e) = res.compile() {
        println!("cargo::error=impossibile compilare le risorse (rc.exe del Windows SDK): {e}");
    }
}
