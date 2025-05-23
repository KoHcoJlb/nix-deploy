use const_format::concatcp;
use tap::Tap;
use tempfile::Builder;

pub fn temp_builder() -> Builder<'static, 'static> {
    Builder::new().tap_mut(|b| {
        b.prefix(concatcp!(env!("CARGO_PKG_NAME"), "-"));
    })
}
