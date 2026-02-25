#[derive(Debug, Clone)]
pub struct ActionDef {
    pub arguments: Vec<ActionArgumentDef>,
    pub rhai_closure: (),
    pub description: Option<String>,
}

impl ActionDef {
    pub fn is_shell(&self) -> bool {
        self.arguments
            .iter()
            .any(|arg| matches!(arg, ActionArgumentDef::ShellAttach))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ActionArgumentDef {
    Runtime,
    ShellAttach,
    OldAppDef,
    AppHistory,
    InstallRequirements,
}
