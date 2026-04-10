use component_macros::define_interface;

define_interface! {
    pub ILogger {
        /// Returns the name of this logger.
        fn name(&self) -> &str;
    }
}
