use anyhow::Error;
use std::sync::Arc;
use tokio::time::Duration;
use wasmtime::{Config, Engine, Linker, Module, Store};
// For this example we want to use the async version of wasmtime_wasi.
// Notably, this version of wasi uses a scheduler that will async yield
// when sleeping in `poll_oneoff`.
use wasmtime_wasi::{tokio::WasiCtxBuilder, WasiCtx};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_init();
    // Create an environment shared by all wasm execution. This contains
    // the `Engine` and the `Module` we are executing.
    let env = Environment::new()?;

    // The inputs to run_wasm are `Send`: we can create them here and send
    // them to a new task that we spawn.
    let inputs1 = Inputs::new(env.clone(), "Gussie");
    let inputs2 = Inputs::new(env.clone(), "Willa");
    let inputs3 = Inputs::new(env, "Sparky");

    // Spawn some tasks. Insert sleeps before run_wasm so that the
    // interleaving is easy to observe.
    let join1 = tokio::task::spawn(async move { run_wasm(inputs1).await });
    let join2 = tokio::task::spawn(async move {
        tokio::time::sleep(Duration::from_millis(750)).await;
        run_wasm(inputs2).await
    });
    let join3 = tokio::task::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1250)).await;
        run_wasm(inputs3).await
    });

    // All tasks should join successfully.
    join1.await??;
    join2.await??;
    join3.await??;
    Ok(())
}

#[derive(Clone)]
struct Environment {
    engine: Engine,
    module: Module,
    linker: Arc<Linker<WasiCtx>>,
}

impl Environment {
    pub fn new() -> Result<Self, Error> {
        let mut config = Config::new();
        // We need this engine's `Store`s to be async, and consume fuel, so
        // that they can co-operatively yield during execution.
        config.async_support(true);
        config.consume_fuel(true);

        let engine = Engine::new(&config)?;
        let module = Module::from_file(&engine, "target/wasm32-wasi/debug/tokio-wasi.wasm")?;

        // A `Linker` is shared in the environment amongst all stores, and this
        // linker is used to instantiate the `module` above. This example only
        // adds WASI functions to the linker, notably the async versions built
        // on tokio.
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::tokio::add_to_linker(&mut linker, |cx| cx)?;

        Ok(Self {
            engine,
            module,
            linker: Arc::new(linker),
        })
    }
}

struct Inputs {
    env: Environment,
    name: String,
}

impl Inputs {
    fn new(env: Environment, name: &str) -> Self {
        Self {
            env,
            name: name.to_owned(),
        }
    }
}

async fn run_wasm(inputs: Inputs) -> Result<(), Error> {
    let span = wasmtime::observ_span!(name = %inputs.name, "run");
    let _ = span.enter();

    let wasi = WasiCtxBuilder::new()
        // Let wasi print to this process's stdout.
        .inherit_stdout()
        // Set an environment variable so the wasm knows its name.
        .env("NAME", &inputs.name)?
        .build();
    let mut store = Store::new(&inputs.env.engine, wasi);

    wasmtime::observ_counter!(stores_created = 1);

    // WebAssembly execution will be paused for an async yield every time it
    // consumes 10000 fuel. Fuel will be refilled u32::MAX times.
    store.out_of_fuel_async_yield(u32::MAX, 10000);

    // Instantiate into our own unique store using the shared linker, afterwards
    // acquiring the `_start` function for the module and executing it.
    let instance = inputs
        .env
        .linker
        .instantiate_async(&mut store, &inputs.env.module)
        .await?;
    wasmtime::observ_counter!(instances_running = 1);
    instance
        .get_typed_func::<(), (), _>(&mut store, "_start")?
        .call_async(&mut store, ())
        .await?;
    wasmtime::observ_counter!(instances_running = (-1));

    Ok(())
}

use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{
    field::{Field, Visit},
    span, subscriber, Event, Metadata, Subscriber,
};
use tracing_subscriber::{
    layer::{Context, Layer, SubscriberExt},
    prelude::*,
    registry::LookupSpan,
    Registry,
};

pub fn tracing_init() {
    Registry::default().with(ObservationLayer::new()).init();
}

struct ObservationStore {
    counters: HashMap<String, Counter>,
}

#[derive(Clone)]
struct ObservationLayer(Arc<Mutex<ObservationStore>>);

impl ObservationLayer {
    fn new() -> Self {
        ObservationLayer(Arc::new(Mutex::new(ObservationStore {
            counters: HashMap::new(),
        })))
    }
    fn interest(meta: &Metadata) -> bool {
        meta.target() == wasmtime::tracing::OBSERV_SPAN
    }
    fn visitor(&self) -> Observation {
        Observation::new(self.clone())
    }
    fn send_counter(&self, record &(String, Counter)) {

    }
}

#[derive(Debug)]
pub struct Counter(i64);
struct Observation {
    contents: Option<(String, Counter)>,
    layer: ObservationLayer,
}

impl Observation {
    pub fn new(layer: ObservationLayer) -> Self {
        Observation {
            contents: None,
            layer: ObservationLayer,
        }
    }
    fn send(&self) {
        if let Some(obs) = self.contents {
            self.layer.send_counter(obs)
        }
    }
}

impl Visit for Observation {
    fn record_i64(&mut self, field: &Field, value: i64) {
        let name = field.name();
        if name.starts_with(wasmtime::tracing::OBSERV_FIELD_PREFIX) {
            let name = name
                .strip_prefix(wasmtime::tracing::OBSERV_FIELD_PREFIX)
                .unwrap()
                .to_owned();
            self.contents = Some((name, Counter(value)));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_i64(field, value as i64)
    }
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl<S> Layer<S> for ObservationLayer
where
    S: Subscriber + std::fmt::Debug + for<'lookup> LookupSpan<'lookup>,
{
    fn register_callsite(&self, meta: &Metadata) -> subscriber::Interest {
        if Self::interest(meta) {
            subscriber::Interest::always()
        } else {
            subscriber::Interest::sometimes()
        }
    }

    fn new_span(&self, _attrs: &span::Attributes, id: &span::Id, ctx: Context<S>) {
        if let Some(meta) = ctx.metadata(id) {
            if Self::interest(&meta) {
                println!("interested in span {:?}: {:?}", id, meta);
            }
        }
    }

    fn on_event(&self, event: &Event, _ctx: Context<S>) {
        if Self::interest(event.metadata()) {
            let mut v = self.visitor();
            event.record(&mut v);
            println!("observation: {:?}", v);
            v.send();
        }
    }
}
