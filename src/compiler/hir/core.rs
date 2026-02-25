use std::{path::PathBuf, sync::Arc};

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub struct HirId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub struct Span {
    pub file: Arc<PathBuf>,
    pub start: u32,
    pub end: u32,
}

#[allow(dead_code)]
impl Span {
    pub fn new(file: Arc<PathBuf>, start: u32, end: u32) -> Self {
        Self { file, start, end }
    }

    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

#[allow(dead_code)]
impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            value: f(self.value),
            span: self.span,
        }
    }

    pub fn as_ref(&self) -> Spanned<&T> {
        Spanned {
            value: &self.value,
            span: self.span.clone(),
        }
    }

    pub fn span(&self) -> &Span {
        &self.span
    }
}

impl<T> From<(T, Span)> for Spanned<T> {
    fn from(v: (T, Span)) -> Self {
        Spanned {
            value: v.0,
            span: v.1,
        }
    }
}
