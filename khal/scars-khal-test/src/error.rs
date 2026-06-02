use scars_fault::*;

#[repr(u8)]
#[derive(PartialEq, Eq, Copy, Clone, Fault)]
pub enum TestErrorKind {
    #[fault("Unknown error")]
    Unknown = 255,
}

#[derive(Fault)]
#[fault("Test harness error: {kind:?}")]
pub struct TestError {
    kind: TestErrorKind,
}

impl TestError {
    pub fn new(kind: TestErrorKind) -> TestError {
        TestError { kind }
    }
}
