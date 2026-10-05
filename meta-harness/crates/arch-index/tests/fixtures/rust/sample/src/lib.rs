pub mod store;
mod util;

use crate::store::Store;
use crate::store::*;
use crate::util::*;
use crate::util::shout as yell;

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

/// Through an alias: `yell` is util::shout.
pub fn yelled(input: &str) -> String {
    yell(input)
}

/// A typed parameter decides the receiver.
pub fn keep(store: &Store) {
    store.save("kept");
}

/// The closure may never run; the dead branches never do.
pub fn later_log() {
    let f = || log("later");
    f();
    if false {
        util::helper();
    }
    #[cfg(any())]
    crate::store::helper();
}

pub async fn fetch() {}

pub fn unfinished() -> u32 {
    std::todo!()
}

macro_rules! wip {
    () => {
        todo!()
    };
}

pub fn drafted() {
    wip!();
}

pub fn wrapper() -> bool {
    fn nested() -> bool {
        todo!()
    }
    nested()
}

pub fn stub() {
    panic!("not implemented yet")
}

pub fn guard(x: u32) -> u32 {
    if x > 1 {
        unreachable!()
    }
    x
}
