pub mod ns16550a;
mod stty;
pub mod virtconsole;

trait SerialOps: Send {
    fn acknowledge_interrupt(&mut self) {}

    fn getchar(&mut self) -> Option<u8>;
    fn putchar(&mut self, c: u8) -> bool;
}
