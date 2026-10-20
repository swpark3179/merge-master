//! Windows: 실행 파일에 아이콘(트레이 아이콘으로도 쓴다)과 버전 정보를 넣는다.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = dir.join("../../src-tauri/icons/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let icon = icon.canonicalize().expect("아이콘(src-tauri/icons/icon.ico)").display().to_string().replace('\\', "/");
    let icon = icon.trim_start_matches("//?/");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<u32> = version.split(['.', '-']).take(3).map(|n| n.parse().unwrap_or(0)).collect();
    let (a, b, c) = (nums[0], nums.get(1).copied().unwrap_or(0), nums.get(2).copied().unwrap_or(0));
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "merge-master"
      VALUE "FileDescription", "Merge Master Agent"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "merge-master-agent"
      VALUE "OriginalFilename", "merge-master-agent.exe"
      VALUE "ProductName", "Merge Master Agent"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("agent.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
}
