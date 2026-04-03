use std::io;

use lumos::{
    app::write_exit_transcript_with_context,
    frontend::tui::{HeroOptions, Model},
};

#[test]
fn write_exit_transcript_adds_context_when_output_fails() {
    let model = Model::new(HeroOptions::default());
    let error = write_exit_transcript_with_context(&mut BrokenWriter, &model)
        .expect_err("writer failures should be wrapped with app context");
    let message = format!("{error:?}");

    assert!(message.contains("failed to write exit transcript"));
}

struct BrokenWriter;

impl io::Write for BrokenWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("synthetic writer failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
