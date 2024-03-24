use crate::{
    event::ActionState,
    packet::{
        acts::packs::{act, log},
        ActComponent, LogComponent, Pack as ActPack,
    },
    sch::Context,
    ActError, ActTask, Pack, Result, StoreAdapter,
};
use async_trait::async_trait;
use futures::executor::block_on;
use wasmtime::{
    component::{Component, Linker, ResourceTable},
    Config, Engine, Result as WitResult, Store,
};
use wasmtime_wasi::preview2::{command, WasiCtx, WasiCtxBuilder, WasiView};

#[async_trait]
impl ActTask for Pack {
    fn init(&self, ctx: &Context) -> Result<()> {
        ctx.task.set_emit_disabled(true);
        Ok(())
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let pack = ctx.scher.cache().store().packages().find(&self.uses)?;
        exec_wit_pack(&pack.file_data, ctx).map_err(|err| ActError::Runtime(err.to_string()))?;

        if ctx.task.state().is_running() {
            ctx.task.set_action_state(ActionState::Completed);
        }
        Ok(())
    }
}

struct PackState<'a> {
    log: LogComponent,
    act: ActComponent<'a>,
    wasi_ctx: WasiCtx,
    wasi_table: ResourceTable,
}

impl<'a> WasiView for PackState<'a> {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.wasi_table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi_ctx
    }
}

fn exec_wit_pack(pack_data: &Vec<u8>, ctx: &Context) -> WitResult<()> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_support(true);
    let engine = Engine::new(&config)?;

    let mut store = Store::new(
        &engine,
        PackState {
            log: LogComponent::new(),
            act: ActComponent::new(ctx),
            wasi_ctx: WasiCtxBuilder::new().inherit_stdio().build(),
            wasi_table: ResourceTable::new(),
        },
    );

    let mut linker = Linker::new(&engine);
    command::add_to_linker(&mut linker)?;
    act::add_to_linker(&mut linker, |s: &mut PackState<'_>| &mut s.act)?;
    log::add_to_linker(&mut linker, |s: &mut PackState<'_>| &mut s.log)?;

    let component = Component::from_binary(&engine, pack_data)?;
    let (pack, _) = block_on(ActPack::instantiate_async(&mut store, &component, &linker))?;
    block_on(pack.call_run(&mut store))?;

    Ok(())
}
