// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Turns on the tests of the real mpv drawing into a plane (`tests/plane.rs`) when
//! `AERITH_TEST_WAYLAND=1`: they need lavapipe, which CI's shell does not hold. Without it they
//! are ignored, not skipped as passed.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(aerith_test_wayland)");
    println!("cargo::rerun-if-env-changed=AERITH_TEST_WAYLAND");
    if std::env::var_os("AERITH_TEST_WAYLAND").is_some_and(|value| value == "1") {
        println!("cargo::rustc-cfg=aerith_test_wayland");
    }
}
