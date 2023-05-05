use crate::Result;
pub use rskafka::{
    client::{partition::Compression, Client, ClientBuilder},
    record::Record,
    //record::RecordAndOffset,
    topic::Topic,
};
use std::collections::BTreeMap;
use time::OffsetDateTime;

pub async fn produce(topic: String, partition: i32, client: &Client, record: Record) -> Result<()> {
    let partition_client = client
        .partition_client(topic.to_owned(), partition)
        .unwrap();
    partition_client
        .produce(vec![record], Compression::Lz4)
        .await
        .unwrap();

    Ok(())
}

pub fn build_record(
    exchange: &str,
    channel: &str,
    inst_id: &[u8],
    data: String,
    partition: String,
) -> Record {
    Record {
        key: Some(inst_id.to_vec()),
        value: Some(data.as_bytes().to_vec()),
        headers: BTreeMap::from([
            ("Exchange".to_owned(), exchange.as_bytes().to_vec()),
            ("Channel".to_owned(), channel.as_bytes().to_vec()),
            ("Partition".to_owned(), partition.as_bytes().to_vec()),
        ]),
        timestamp: OffsetDateTime::now_utc(),
    }
}
