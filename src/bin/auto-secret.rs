use clap::Parser;
use futures_util::StreamExt;
use k8s_openapi::{api::core::v1::Secret, ByteString};
use kube::{
    api::{Api, Patch, PatchParams, Resource},
    runtime::{
        controller::{Action, Config, Controller},
        watcher,
    },
    Client, ResourceExt,
};
use log::{debug, info, warn};
use randstr::{randstr, RandStrBuilder};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};
use thiserror::Error;
use tokio::time::Duration;

macro_rules! app_id {
    () => {
        "auto-secret.k8s.eboland.de"
    };
    ($name:tt) => {
        concat!(app_id!(), "/", $name)
    };
}

#[derive(Error, Debug)]
enum Error {
    #[error("serde error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("kube error: {0}")]
    Kube(#[from] kube::Error),
    #[error("randstr error: {0}")]
    RandStr(#[from] randstr::Error),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    #[serde(default)]
    upper: bool,
    #[serde(default)]
    lower: bool,
    #[serde(default, alias = "letters")]
    letter: bool,
    #[serde(default, alias = "digits")]
    digit: bool,
    #[serde(default, alias = "symbols")]
    symbol: bool,
    #[serde(default, alias = "whitespaces")]
    whitespace: bool,
    custom: Option<String>,

    #[serde(default)]
    must_upper: bool,
    #[serde(default)]
    must_lower: bool,
    #[serde(default, alias = "must_letters")]
    must_letter: bool,
    #[serde(default, alias = "must_digits")]
    must_digit: bool,
    #[serde(default, alias = "must_symbols")]
    must_symbol: bool,
    #[serde(default, alias = "must_whitespaces")]
    must_whitespace: bool,
    must_custom: Option<String>,

    #[serde(default)]
    length: Option<usize>,
}

#[derive(Debug, Deserialize)]
enum Preset {
    #[serde(rename = "all", alias = "default")]
    All,
    #[serde(rename = "digit", alias = "digits")]
    Digit,
    #[serde(rename = "letter", alias = "letters")]
    Letter,
    #[serde(rename = "upper")]
    Upper,
    #[serde(rename = "lower")]
    Lower,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Settings {
    Preset(Preset),
    Options(Options),
}

#[derive(Parser)]
struct Opts {
    #[clap(short, long, default_value = "32")]
    default_length: usize,
}

impl Settings {
    fn apply(&self, builder: &mut RandStrBuilder) {
        match self {
            Settings::Preset(Preset::All) => {
                builder.all();
            }
            Settings::Preset(Preset::Digit) => {
                builder.digit();
            }
            Settings::Preset(Preset::Letter) => {
                builder.letter();
            }
            Settings::Preset(Preset::Upper) => {
                builder.upper();
            }
            Settings::Preset(Preset::Lower) => {
                builder.lower();
            }
            Settings::Options(o) => {
                if o.upper {
                    builder.upper();
                }
                if o.lower {
                    builder.lower();
                }
                if o.letter {
                    builder.letter();
                }
                if o.digit {
                    builder.digit();
                }
                if o.symbol {
                    builder.symbol();
                }
                if o.whitespace {
                    builder.whitespace();
                }
                if let Some(custom) = &o.custom {
                    builder.custom(custom);
                }

                if o.must_upper {
                    builder.must_upper();
                }
                if o.must_lower {
                    builder.must_lower();
                }
                if o.must_letter {
                    builder.must_letter();
                }
                if o.must_digit {
                    builder.must_digit();
                }
                if o.must_symbol {
                    builder.must_symbol();
                }
                if o.must_whitespace {
                    builder.must_whitespace();
                }
                if let Some(must_custom) = &o.must_custom {
                    builder.must_custom(must_custom);
                }
                if let Some(len) = o.length {
                    builder.len(len);
                }
            }
        }
    }
}

fn gen_credential(
    opts: &Opts,
    settings: &Settings,
) -> Result<ByteString, Error> {
    let mut builder = randstr();
    builder.len(opts.default_length);

    settings.apply(&mut builder);

    Ok(ByteString(builder.try_build()?.generate().into_bytes()))
}

struct Context {
    client: Client,
    opts: Opts,
}

async fn reconcile(
    secret: Arc<Secret>,
    ctx: Arc<Context>,
) -> Result<Action, Error> {
    let client = ctx.client.clone();
    let name = secret.name_any();
    let ns = secret.namespace().unwrap();
    let api = Api::<Secret>::namespaced(client.clone(), &ns);
    let mut secret = Arc::unwrap_or_clone(secret);

    let old_data = secret.data.take().unwrap_or_default();

    let Some(settings) = secret
        .meta()
        .annotations
        .as_ref()
        .and_then(|a| a.get(app_id!("gen")))
    else {
        return Ok(Action::await_change());
    };

    let data: BTreeMap<String, ByteString> = serde_yaml::from_str::<
        BTreeMap<String, Settings>,
    >(settings)?
    .iter()
    .filter(|(k, _)| !old_data.contains_key(*k))
    .map(|(k, v)| {
        Ok::<(_, _), Error>((k.to_string(), gen_credential(&ctx.opts, v)?))
    })
    .collect::<Result<_, Error>>()?;

    debug!("Generated data: {:?}", data);
    api.patch(
        &name,
        &PatchParams::apply(app_id!()),
        &Patch::Merge([("data", data)].into_iter().collect::<BTreeMap<_, _>>()),
    )
    .await?;
    Ok(Action::requeue(Duration::from_secs(300)))
}

fn error_policy(
    _object: Arc<Secret>,
    error: &Error,
    _client: Arc<Context>,
) -> Action {
    match error {
        Error::Kube(_) => Action::requeue(Duration::from_secs(5)),
        _ => Action::await_change(),
    }
}

#[cfg(debug_assertions)]
fn init_logger() {
    pretty_env_logger::init();
}

#[cfg(not(debug_assertions))]
fn init_logger() {
    use structured_logger::{async_json::new_writer, Builder};

    Builder::with_level("info")
        .with_target_writer("*", new_writer(tokio::io::stdout()))
        .init();
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let opts = Opts::parse();

    init_logger();
    let client = Client::try_default().await?;

    let api = Api::<Secret>::all(client.clone());

    eprintln!(
        "Starting auto-secret-operator version {}",
        env!("CARGO_PKG_VERSION")
    );

    let config = Config::default().concurrency(2);

    Controller::new(api, watcher::Config::default())
        .with_config(config)
        .shutdown_on_signal()
        .run(reconcile, error_policy, Arc::new(Context { client, opts }))
        .for_each(|res| async move {
            match res {
                Ok((o, _a)) => info!(
                    "reconciled {}/{}",
                    o.namespace.as_deref().unwrap_or("<unknown>"),
                    o.name
                ),
                Err(kube::runtime::controller::Error::ReconcilerFailed(
                    e,
                    _,
                )) => {
                    warn!("reconcile failed: {}", e);
                    debug!("reconcile failed: {:?}", e);
                }
                Err(e) => {
                    warn!("reconcile failed: {}", e);
                    debug!("reconcile failed: {:?}", e);
                }
            }
        })
        .await;
    info!("controller terminated");
    Ok(())
}
