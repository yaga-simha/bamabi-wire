use bamabi_macros::ModComponent;

#[derive(Clone, ModComponent)]
pub struct NamedComp {
    pub title: String,
}

#[derive(Clone, ModComponent)]
pub struct OuterInventory {
    pub bad_items: Vec<NamedComp>,
}

fn main() {}
