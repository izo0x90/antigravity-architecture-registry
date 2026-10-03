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
