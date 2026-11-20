use bamabi_macros::AbiPod;

#[repr(u32)]
#[derive(Copy, Clone, AbiPod)]
pub enum StateEnum {
    Idle = 0,
    Moving = 1,
}

fn main() {}
