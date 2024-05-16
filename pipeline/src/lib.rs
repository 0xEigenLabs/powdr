//! The main powdr lib, used to compile from assembly to PIL

#![deny(clippy::print_stdout)]

use std::marker::{Send, Sync};

pub mod pipeline;
pub mod test_util;
pub mod util;
pub mod verify;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub use pipeline::Pipeline;

pub use powdr_backend::{BackendType, Proof};
use powdr_executor::witgen::QueryCallback;

use powdr_number::FieldElement;

#[derive(Clone)]
pub struct HostContext<T> {
    // Shared mutable state
    pub data: Arc<Mutex<BTreeMap<u32, Vec<u8>>>>,
    // The callable part of the Database, implemented via a closure
    pub cb: Arc<dyn QueryCallback<T>>,
}

impl<T: FieldElement> HostContext<T> {
    pub fn new() -> Self
    {
        let data_arc = Arc::new(Mutex::new(BTreeMap::<u32, Vec<u8>>::new()));
        let data_for_closure = data_arc.clone();
        
        let cb = Arc::new(move |query: &str| -> Result<Option<T>, String> {
            let mut map = data_for_closure.lock().unwrap();
            map.insert(666, vec![
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06
            ]);
            Err("Unsupported query".to_string())
        });

        Self {
            data: data_arc,
            cb,
        }
    }
}

// TODO at some point, we could also just pass evaluator::Values around - would be much faster.
pub fn parse_query(query: &str) -> Result<(&str, Vec<&str>), String> {
    // We are expecting an enum value
    if let Some(paren) = query.find('(') {
        let name = &query[..paren];
        let data = query[paren + 1..].strip_suffix(')').ok_or_else(|| {
            format!(
                "Error parsing query input \"{query}\". Could not find closing ')' in enum data."
            )
        })?;
        Ok((name, data.split(',').map(|s| s.trim()).collect::<Vec<_>>()))
    } else {
        Ok((query, vec![]))
    }
}

pub fn access_element<T: FieldElement>(
    name: &str,
    elements: &[T],
    index_str: &str,
) -> Result<Option<T>, String> {
    let index = index_str
        .parse::<usize>()
        .map_err(|e| format!("Error parsing index: {e})"))?;
    let value = elements.get(index).cloned();
    if let Some(value) = value {
        log::trace!("Query for {name}: Index {index} -> {value}");
        Ok(Some(value))
    } else {
        Err(format!(
            "Error accessing {name}: Index {index} out of bounds {}",
            elements.len()
        ))
    }
}

pub fn serde_data_to_query_callback<T: FieldElement, S: serde::Serialize + Send + Sync>(
    channel: u32,
    data: &S,
) -> impl QueryCallback<T> {
    let bytes = serde_cbor::to_vec(&data).unwrap();
    move |query: &str| -> Result<Option<T>, String> {
        let (id, data) = parse_query(query)?;
        match id {
            "None" => Ok(None),
            "DataIdentifier" => {
                let [index, cb_channel] = data[..] else {
                    panic!()
                };
                let cb_channel = cb_channel
                    .parse::<u32>()
                    .map_err(|e| format!("Error parsing callback data channel: {e})"))?;

                if channel != cb_channel {
                    return Err("Callback channel mismatch".to_string());
                }

                let index = index
                    .parse::<usize>()
                    .map_err(|e| format!("Error parsing index: {e})"))?;

                // query index 0 means the length
                Ok(Some(match index {
                    0 => (bytes.len() as u64).into(),
                    index => (bytes[index - 1] as u64).into(),
                }))
            }
            _ => Err(format!("Unsupported query: {query}")),
        }
    }
}

pub fn inputs_to_query_callback<T: FieldElement>(inputs: Vec<T>) -> impl QueryCallback<T> {
    move |query: &str| -> Result<Option<T>, String> {
        let (id, data) = parse_query(query)?;
        match id {
            "None" => Ok(None),
            "Input" => {
                assert_eq!(data.len(), 1);
                access_element("prover inputs", &inputs, data[0])
            }
            _ => Err(format!("Unsupported query: {query}")),
        }
    }
}

#[allow(clippy::print_stdout)]
pub fn handle_simple_queries_callback<'a, T: FieldElement>() -> impl QueryCallback<T> + 'a {
    move |query: &str| -> Result<Option<T>, String> {
        let (id, data) = parse_query(query)?;
        match id {
            "None" => Ok(None),
            "Output" => {
                assert_eq!(data.len(), 2);
                let fd = data[0]
                    .parse::<u32>()
                    .map_err(|e| format!("Invalid fd: {e}"))?;
                let byte = data[1]
                    .parse::<u8>()
                    .map_err(|e| format!("Invalid char to print: {e}"))?
                    as char;
                match fd {
                    1 => print!("{}", byte),
                    2 => eprint!("{}", byte),
                    _ => return Err(format!("Unsupported file descriptor: {fd}")),
                }
                Ok(Some(0.into()))
            }
            "Hint" => {
                assert_eq!(data.len(), 1);
                Ok(Some(T::from_str(data[0]).unwrap()))
            }
            _ => Err(format!("Unsupported query: {query}")),
        }
    }
}
