use crate::{
    PackageInfo, Principal, Result, data::Package, query::Query, scheduler::Runtime,
    store::PageData,
};
use std::sync::Arc;
use tracing::instrument;

/// `pack:ls` — list the packages in the catalogue.
pub(crate) const LS: &str = "pack:ls";
/// `pack:get` — read one package definition.
pub(crate) const GET: &str = "pack:get";
/// `pack:publish` — write a package definition into the catalogue.
pub(crate) const PUBLISH: &str = "pack:publish";
/// `pack:rm` — delete a package definition.
pub(crate) const RM: &str = "pack:rm";

#[derive(Clone)]
pub struct PackageExecutor {
    runtime: Arc<Runtime>,
    principal: Arc<Principal>,
}

impl PackageExecutor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Arc<Principal>) -> Self {
        Self {
            runtime: rt.clone(),
            principal: principal.clone(),
        }
    }

    #[instrument(skip(self, pack), fields(id = %pack.id))]
    pub async fn publish(&self, pack: &Package) -> Result<bool> {
        self.principal.check(PUBLISH)?;
        super::publish_package(&self.runtime, pack).await
    }

    #[instrument(skip(self, q))]
    pub async fn list(&self, q: &Query) -> Result<PageData<PackageInfo>> {
        self.principal.check(LS)?;
        super::list_infos(self.runtime.store().packages(), q).await
    }

    #[instrument(skip(self), fields(id = %id))]
    pub async fn get(&self, id: &str) -> Result<PackageInfo> {
        self.principal.check(GET)?;
        super::get_info(self.runtime.store().packages(), id).await
    }

    #[instrument(skip(self), fields(id = %id))]
    pub async fn rm(&self, id: &str) -> Result<bool> {
        self.principal.check(RM)?;
        let ret = self.runtime.store().packages().delete(id).await?;
        if ret {
            self.runtime.schema_cache().invalidate_package(id);
        }
        Ok(ret)
    }
}
