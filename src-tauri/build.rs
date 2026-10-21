fn main() {
    // Windows(MSVC): 메인 스레드 스택을 8MB로(기본 1MB, 예약만 늘고 쓰는 만큼만 잡힌다). Tauri는 async command의 future를
    // 메인 스레드에서 만들어 값으로 넘겨, 큰 future 하나로도 스택이 넘치면 앱이 메시지 없이 꺼진다.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
    tauri_build::build()
}
