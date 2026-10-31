fn main() -> std::process::ExitCode {
    stargauge::platform::init_console();
    stargauge::platform::install_ctrlc();
    let code = stargauge::run();
    stargauge::platform::pause_if_own_console();
    code
}
