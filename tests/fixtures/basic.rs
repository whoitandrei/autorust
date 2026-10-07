use std::io;
use std::fmt;

pub fn greet() -> String {
    let s = "привет, мир 🦀";
    let a = 1;
    return s.to_string();
}

pub fn empty(v: &Vec<i32>) -> bool {
    v.len() == 0
}

pub fn clone_it(x: String) -> String {
    let y = x.clone();
    y
}

pub fn add(a: i32, b: i32) -> i32 {
    return a + b;
}
