use crate::env::Environment;

mod array;
mod console;

impl Environment {
    pub fn init(&mut self) {
        let mut modules = self.modules.write().expect("modules lock poisoned");

        modules.push(Box::new(console::ConsoleModule::new()));
        modules.push(Box::new(array::ArrayModule::new()));
    }
}
