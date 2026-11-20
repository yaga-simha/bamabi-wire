use bamabi_macros::ModComponent;

#[repr(C)]
#[derive(Copy, Clone, ModComponent)]
#[mod_component(no_pod)]
pub struct ExplicitNoPod {
    pub value: u64,
}

fn main() {
    bamabi_sys::assert_abi_pod::<ExplicitNoPod>();
}
