use super::Enviroment;

mod act;
mod array;
mod console;
mod env;

impl Enviroment {
    pub fn init(&mut self) {
        let mut modules = self.modules.write().unwrap();
        modules.push(Box::new(console::Console::new()));
        modules.push(Box::new(array::Array::new()));
        modules.push(Box::new(act::ActPackage::new()));
        modules.push(Box::new(env::Env::new()));
    }
}
