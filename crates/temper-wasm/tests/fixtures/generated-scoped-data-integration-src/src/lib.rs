use temper_wasm_sdk::prelude::*;

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

const CUSTOMER_ID: &str = "018f1f80-7b2d-7000-8000-000000000076";

temper_module! {
    fn generated_scoped_data(ctx: Context) -> Result<Value> {
        let mut client = CustomerClient::new();
        let created_sequence = if ctx.entity_id == "worker-before-restart" {
            let id = CustomerId(CUSTOMER_ID.into());
            let name = String::from("generated-scoped-client");
            let create = CustomerCreate::new(CustomerIdRef::from(&id)).with_name(&name);
            client
                .create(&create)
                .map_err(|error| error.to_string())?
                .commit
                .sequence
        } else {
            client
                .get(CUSTOMER_ID)
                .map_err(|error| error.to_string())?
                .sequence
        };
        let read = client
            .get(CUSTOMER_ID)
            .map_err(|error| error.to_string())?;
        Ok(json!({
            "customer_id": read.value.id,
            "name": read.value.name,
            "created_sequence": created_sequence,
            "read_sequence": read.sequence,
        }))
    }
}
