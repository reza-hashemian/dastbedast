//! Desktop entry point. On Android the system loads the library and calls `run` itself.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dastbedast_lib::run()
}
