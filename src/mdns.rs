use futures::channel::mpsc;
use std::any::Any;
use std::sync::Arc;
use std::time::Duration;
use zeroconf::prelude::*;
use zeroconf::{MdnsBrowser, ServiceDiscovery, ServiceType};

#[derive(Debug, Clone)]
pub struct Discovered {
    pub host: String,
    pub port: u16,
}

pub async fn find_companion(timeout: Duration) -> Result<Vec<Discovered>, &'static str> {
    let service_type = ServiceType::with_sub_types(&"companion-link", &"tcp", vec![]).expect("invalid service type");

    let mut browser = MdnsBrowser::new(service_type);

    let (tx, mut rx) = mpsc::unbounded();
    browser.set_context(Box::new(Some(Arc::new(tx))));
    browser.set_service_discovered_callback(Box::new(on_service_discovered));

    let event_loop = browser
        .browse_services()
        .expect("Failed to browse for services");

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        event_loop.poll(Duration::from_secs(1)).unwrap();
        if tokio::time::Instant::now() > deadline {
            break;
        }
    }

    let mut results = Vec::new();
    while let Ok(discovered) = rx.try_recv() {
        results.push(discovered);
    }

    if !results.is_empty() {
        return Ok(results);
    } else {
        return Err("No _companion-link._tcp service found");
    }
}


fn on_service_discovered(
    result: zeroconf::Result<ServiceDiscovery>,
    context: Option<Arc<dyn Any>>,
) {
    if context.is_some() {
        let discovered = Discovered {
            host: result.as_ref().unwrap().host_name().clone(),
            port: *result.as_ref().unwrap().port(),
        };
        println!("{:?}", discovered);
        context.unwrap().downcast_ref::<Option<Arc<mpsc::UnboundedSender<Discovered>>>>().unwrap().as_ref().unwrap().unbounded_send(discovered).unwrap();
    }
}
