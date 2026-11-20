use bamabi_macros::ModComponent;

#[repr(C, u8)]
#[derive(Copy, Clone, ModComponent)]
pub enum EnumNotAbiPod {
    Idle = 0,
    Move(f32) = 1,
}

fn main() {
    bamabi_sys::assert_abi_pod::<EnumNotAbiPod>();
}
