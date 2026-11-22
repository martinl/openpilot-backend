pub use super::_entities::bootlogs::{self, ActiveModel, Column, Entity, Model as BM};
use crate::loco_rs::model::ModelResult;
use chrono::prelude::Utc;
use sea_orm::{entity::prelude::*, ActiveValue, TransactionTrait};

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    // extend activemodel below (keep comment for generators)
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        if insert {
            this.created_at = ActiveValue::Set(Utc::now().naive_utc());
            this.updated_at = ActiveValue::Set(Utc::now().naive_utc());
            Ok(this)
        } else {
            // update time
            this.updated_at = ActiveValue::Set(Utc::now().naive_utc());
            Ok(this)
        }
    }
}

/// Implementation of the `Model` struct for routes.
impl BM {
    pub async fn add_bootlog(
        db: &DatabaseConnection,
        dongle_id: &str,
        bootlog_download_url: &str,
        unlog_url: &str,
        date_time: &str,
    ) -> ModelResult<Self> {
        let txn = match db.begin().await {
            Ok(txn) => {
                tracing::debug!("Transaction began");
                txn
            }
            Err(e) => {
                tracing::error!("Failed to begin the transaction: {:?}", e);
                return Err(e.into());
            }
        };
        let bootlog = ActiveModel {
            dongle_id: ActiveValue::Set(dongle_id.to_owned()),
            bootlog_url: ActiveValue::Set(bootlog_download_url.to_owned()),
            unlog_url: ActiveValue::Set(unlog_url.to_owned()),
            date_time: ActiveValue::Set(date_time.to_owned()),
            ..Default::default()
        }
        .insert(&txn)
        .await;
        match bootlog {
            Ok(bootlog) => {
                tracing::debug!("Bootlog Added to database");
                txn.commit().await?;
                Ok(bootlog)
            }
            Err(e) => {
                tracing::error!("Failed to add bootlog to database: {:?}", e);
                Err(e.into())
            }
        }
    }

    pub async fn find_device_bootlogs(
        db: &DatabaseConnection,
        dongle_id: &String,
    ) -> ModelResult<Vec<BM>> {
        let routes = Entity::find()
            .filter(Column::DongleId.eq(dongle_id))
            .all(db)
            .await?;
        Ok(routes)
    }
}
