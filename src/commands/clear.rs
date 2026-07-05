use std::io::Write;

use tinyharness_ui::output::Output;

use tinyharness_ui::style::CLEAR_SCREEN;

pub fn execute(out: &mut Output) {
    let _ = write!(out, "{CLEAR_SCREEN}");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[test]
    fn clear_writes_escape_sequence() {
        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        struct W(Arc<Mutex<Vec<u8>>>);
        impl Write for W {
            fn write(&mut self, d: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(d);
                Ok(d.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = Output::new(Box::new(W(buf.clone())));
        execute(&mut out);
        let s = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(s.contains(CLEAR_SCREEN));
    }
}
