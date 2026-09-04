use super::Enviroment;

mod act;
mod array;
mod console;
mod env;
mod os;
mod sealed;
mod step;
mod vars;

impl Enviroment {
    pub fn init(&mut self) {
        let mut modules = self.modules.write();

        modules.push(Box::new(console::ConsoleModule::new()));
        modules.push(Box::new(array::ArrayModule::new()));
        modules.push(Box::new(act::ActJsModule::new()));
        modules.push(Box::new(step::StepModule::new()));
        modules.push(Box::new(env::ProcEnv::new()));
        modules.push(Box::new(vars::UserVars::new(self)));
        modules.push(Box::new(sealed::SealedModule::new()));
        modules.push(Box::new(os::Os::new()));

        let mut user_vars = self.user_vars.write();
        user_vars.push(Box::new(vars::secrets::SecretsVar));
    }
}
