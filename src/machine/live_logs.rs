use super::MachineIdentity;
use crate::client;
use serde::Serialize;
use std::io::Read;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

const STEP_BYTES: usize = 16 * 1024;
const JOB_BYTES: usize = 128 * 1024;
const TRUNCATED: &str = "\n[Output truncated]\n";

#[derive(Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct LiveStep {
    pub name: String,
    pub exit_code: Option<i32>,
    pub output: String,
    #[serde(skip)]
    truncated: bool,
}

pub(super) type Logs = Arc<Mutex<Vec<LiveStep>>>;

pub(super) fn begin(logs: &Logs, name: &str) {
    logs.lock().unwrap().push(LiveStep {
        name: name.into(),
        exit_code: None,
        output: String::new(),
        truncated: false,
    });
}

pub(super) fn append(logs: &Logs, index: usize, text: &str) {
    let mut steps = logs.lock().unwrap();
    let used: usize = steps.iter().map(|step| step.output.len()).sum();
    // Reserve a truncation notice for each possible remaining step.
    let remaining = JOB_BYTES.saturating_sub(used + (20 - steps.len()) * TRUNCATED.len());
    let step = &mut steps[index];
    if step.truncated {
        return;
    }
    let budget = (STEP_BYTES - step.output.len()).min(remaining);
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let available = budget.saturating_sub(TRUNCATED.len());
    let mut end = text.len().min(available);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    step.output.push_str(&text[..end]);
    if end < text.len() {
        step.output.push_str(TRUNCATED);
        step.truncated = true;
    }
}

pub(super) fn drain<R: Read + Send + 'static>(
    mut stream: R,
    logs: Logs,
    index: usize,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut pending = Vec::new();
        let mut buffer = [0u8; 8192];
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            pending.extend_from_slice(&buffer[..count]);
            // Keep incomplete UTF-8 across reads; never revise an already uploaded prefix.
            let mut consumed = 0;
            while consumed < pending.len() {
                match std::str::from_utf8(&pending[consumed..]) {
                    Ok(text) => {
                        append(&logs, index, text);
                        consumed = pending.len();
                    }
                    Err(error) => {
                        let end = consumed + error.valid_up_to();
                        append(
                            &logs,
                            index,
                            std::str::from_utf8(&pending[consumed..end]).unwrap(),
                        );
                        consumed = end;
                        if let Some(length) = error.error_len() {
                            append(&logs, index, "�");
                            consumed += length;
                        } else {
                            break;
                        }
                    }
                }
            }
            pending.drain(..consumed);
        }
        if !pending.is_empty() {
            append(&logs, index, &String::from_utf8_lossy(&pending));
        }
    })
}

pub(super) fn upload(
    machine: MachineIdentity,
    id: String,
    token: String,
    logs: Logs,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut previous = Vec::new();
        let mut warned = false;
        while !stop.load(Ordering::Relaxed) {
            let snapshot = logs.lock().unwrap().clone();
            if !snapshot.is_empty() && snapshot != previous {
                let response = client().and_then(|client| {
                    client
                        .post(format!(
                            "{}/machine-enrollment/jobs/{id}/logs",
                            machine.api_url
                        ))
                        .bearer_auth(&machine.credential)
                        .timeout(Duration::from_secs(5))
                        .json(&serde_json::json!({ "assignmentToken": token, "steps": snapshot }))
                        .send()
                        .map_err(|_| "live log upload failed".to_owned())
                });
                match response {
                    Ok(response) if response.status().is_success() => {
                        previous = snapshot;
                        warned = false;
                    }
                    Ok(response) if matches!(response.status().as_u16(), 401 | 404 | 409) => break,
                    _ => {
                        if !warned {
                            eprintln!("Live log upload interrupted; retrying while the job runs.");
                            warned = true;
                        }
                    }
                }
            }
            thread::park_timeout(Duration::from_secs(2));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_is_bounded_and_marks_truncation() {
        let logs = Logs::default();
        for index in 0..20 {
            begin(&logs, "Step");
            append(&logs, index, &"é".repeat(STEP_BYTES));
        }
        let steps = logs.lock().unwrap();
        assert!(
            steps
                .iter()
                .all(|step| step.output.len() <= STEP_BYTES && step.output.ends_with(TRUNCATED))
        );
        assert!(steps.iter().map(|step| step.output.len()).sum::<usize>() <= JOB_BYTES);
    }

    #[test]
    fn split_utf8_preserves_append_only_output() {
        struct Bytes(std::vec::IntoIter<u8>);
        impl Read for Bytes {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if let Some(byte) = self.0.next() {
                    out[0] = byte;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
        }
        let logs = Logs::default();
        begin(&logs, "UTF8");
        drain(
            Bytes("hello 🦀\n".as_bytes().to_vec().into_iter()),
            logs.clone(),
            0,
        )
        .join()
        .unwrap();
        assert_eq!(logs.lock().unwrap()[0].output, "hello 🦀\n");
    }
}
