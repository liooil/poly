// Poly Rust interpreter demo.
//
// Run with:  poly run rust_demo.rs   (or  poly rust_demo.rs)
//
// The Poly Rust runtime is an experimental in-process interpreter: the source
// is parsed with rust-analyzer's parser and executed without rustc, Cargo,
// LLVM, or any generated executable. A subset of Rust is supported — see
// `poly --help` → "Rust (experimental interpreter)".

fn fib(n: i64) -> i64 {
    if n <= 1 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

fn main() {
    println!("Hello from the Poly Rust interpreter!");

    let mut sum = 0;
    let mut i = 0;
    while i <= 10 {
        sum = sum + i;
        i = i + 1;
    }
    println!("sum 0..10 = {sum}");

    let n = 20;
    println!("fib({n}) = {}", fib(n));
    println!("gcd(48, 18) = {}", gcd(48, 18));

    let pi: f64 = 3.14159;
    let label = "poly";
    println!("{label} {pi} {}", 10 > 5);
}
