//! Appunti di sistema (copia e incolla fra le app).

use alloc::string::String;

static mut CLIP: String = String::new();

pub fn set(s: &str) {
    unsafe { *core::ptr::addr_of_mut!(CLIP) = String::from(s) };
}

pub fn get() -> String {
    unsafe { (*core::ptr::addr_of!(CLIP)).clone() }
}
