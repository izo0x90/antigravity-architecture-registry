pub mod store;
mod util;

use crate::store::Store;

pub trait Step {
    fn apply(&self, input: &str) -> String;
}

pub struct Upper;

impl Step for Upper {
    fn apply(&self, input: &str) -> String {
        util::shout(input)
    }
}

pub fn run(input: &str) -> String {
    let store = Store::new();
    let out = Upper.apply(input);
    store.save(&out);
    log(&out);
    out
}

fn log(msg: &str) {
    println!("{msg}");
}

pub fn later() -> u32 {
    todo!()
}

fn ambiguous() {
    helper();
}
