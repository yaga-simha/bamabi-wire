use bamabi_macros::ModComponent;

#[derive(Clone, ModComponent)]
pub struct NamedComp {
    pub title: String,
}

fn main() {
    bamabi_sys::assert_abi_pod::<NamedComp>();
}
