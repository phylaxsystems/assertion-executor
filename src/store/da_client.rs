use jsonrpsee::{
    core::client::{
        ClientT,
        Error as ClientError,
    },
    http_client::{
        HttpClient,
        HttpClientBuilder,
    },
};

use crate::primitives::{
    Bytes,
    B256,
};

/// A client for interacting with the DA layer
/// This client is responsible for fetching bytecode from the DA layer
///
/// ``` no_run
/// use assertion_executor::{store::DaClient, primitives::B256};
///
/// #[tokio::main]
/// async fn main() {
///
///     let da_client = DaClient::new("http://localhost:3030").unwrap();
///     let assertion_id = B256::random();
///     let bytecode = da_client.fetch_assertion_bytecode(assertion_id).await.unwrap();
/// }         
#[derive(Debug)]
pub struct DaClient {
    client: HttpClient,
}

#[derive(Debug, thiserror::Error)]
pub enum DaClientError {
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

impl DaClient {
    /// Create a new DA client
    pub fn new(da_url: &str) -> Result<Self, DaClientError> {
        let client = HttpClientBuilder::default().build(da_url)?;

        Ok(Self { client })
    }

    /// Fetch the bytecode for the given assertion id from the DA layer
    pub async fn fetch_assertion_bytecode(
        &self,
        assertion_id: B256,
    ) -> Result<Bytes, DaClientError> {
        let code = self
            .client
            .request::<Bytes, &[String]>("da_get_assertion", &[assertion_id.to_string()])
            .await?;

        Ok(code)
    }

    /// Submit the assertion bytecode to the DA layer
    pub async fn submit_assertion(
        &self,
        assertion_id: B256,
        code: Bytes,
    ) -> Result<(), DaClientError> {
        self.client
            .request::<(), (String, String)>(
                "da_submit_assertion",
                (assertion_id.to_string(), code.to_string()),
            )
            .await?;
        Ok(())
    }
}
