pub mod ns16550a;
mod stty;

trait SerialOps: Send {
    fn getchar(&mut self) -> Option<u8>;
    fn putchar(&mut self, c: u8) -> bool;
}
