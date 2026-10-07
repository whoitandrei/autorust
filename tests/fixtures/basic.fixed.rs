
pub fn greet() -> String {
    let s = "привет, мир 🦀";
    let _a = 1;
    s.to_string()
}

pub fn empty(v: &Vec<i32>) -> bool {
    v.is_empty()
}

pub fn clone_it(x: String) -> String {
    
    x.clone()
}

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
