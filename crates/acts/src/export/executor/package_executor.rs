use crate::{
    PackageInfo, Principal, Result, data::Package, query::Query, scheduler::Runtime, store::PageData,
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
        let ret = self.runtime.cache().store().publish(pack).await?;
        if ret {
            self.runtime.schema_cache().invalidate_package(&pack.id);
        }
        Ok(ret)
    }

    #[instrument(skip(self, q))]
    pub async fn list(&self, q: &Query) -> Result<PageData<PackageInfo>> {
        self.principal.check(LS)?;
        match self.runtime.cache().store().packages().query(q).await {
            Ok(packages) => Ok(PageData {
                count: packages.count,
                page_size: packages.page_size,
                page_count: packages.page_count,
                page_num: packages.page_num,
                rows: packages.rows.iter().map(|m| m.into()).collect(),
            }),
            Err(err) => Err(err),
        }
    }

    #[instrument(skip(self), fields(id = %id))]
    pub async fn get(&self, id: &str) -> Result<PackageInfo> {
        self.principal.check(GET)?;
        let package = &self.runtime.cache().store().packages().find(id).await?;
        Ok(package.into())
    }

    #[instrument(skip(self), fields(id = %id))]
    pub async fn rm(&self, id: &str) -> Result<bool> {
        self.principal.check(RM)?;
        let ret = self.runtime.cache().store().packages().delete(id).await?;
        if ret {
            self.runtime.schema_cache().invalidate_package(id);
        }
        Ok(ret)
    }
}
