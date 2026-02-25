#[unsafe(no_mangle)]
pub extern "C" fn arth_demo_add_i64(a: i64, b: i64) -> i64 {
    a + b
}

#[unsafe(no_mangle)]
pub extern "C" fn arth_demo_mul_f64(a: f64, b: f64) -> f64 {
    a * b
}

