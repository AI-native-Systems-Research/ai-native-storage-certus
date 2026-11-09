use component_macros::define_interface;

define_interface! {
    pub IGreeter {
        fn greeting_prefix(&self) -> &str;
    }
}
