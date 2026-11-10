use component_macros::define_interface;

define_interface! {
    pub ILogger {
        fn error(&self, msg: &str);
        fn warn(&self, msg: &str);
        fn info(&self, msg: &str);
        fn debug(&self, msg: &str);
    }
}
