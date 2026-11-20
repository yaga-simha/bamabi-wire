use bamabi_macros::ModComponent;
use bamabi_sys::FfiOption;

#[repr(C)]
#[derive(Copy, Clone, ModComponent)]
pub struct BadFfiOptionBool {
    pub bad: FfiOption<bool>,
}

fn main() {}
