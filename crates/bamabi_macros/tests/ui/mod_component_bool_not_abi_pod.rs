use bamabi_macros::ModComponent;

#[repr(C)]
#[derive(Copy, Clone, ModComponent)]
pub struct BoolComp {
    pub flag: bool,
    pub count: u32,
}

fn main() {
    bamabi_sys::assert_abi_pod::<BoolComp>();
}
