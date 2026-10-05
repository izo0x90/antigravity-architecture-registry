pub struct Store {
    items: Vec<String>,
}

impl Store {
    pub fn new() -> Self {
        Store { items: Vec::new() }
    }

    pub fn save(&self, item: &str) {
        let _ = item;
        self.check();
    }

    fn check(&self) {}
}

pub fn helper() {}

/// `s` comes from a loop: its type isn't known, so the call is unproven.
pub fn store_each(items: Vec<Store>) {
    for s in items {
        s.save("x");
    }
}
