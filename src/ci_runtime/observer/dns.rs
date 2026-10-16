//! Each async DNS lookup belongs to the transfer's current scoped runtime.
use hickory_resolver::{
    TokioResolver,
    config::{LookupIpStrategy, ResolveHosts},
};

#[derive(Default)]
pub(super) struct OwnedResolver {
    #[cfg(test)]
    pub(super) config: Option<hickory_resolver::config::ResolverConfig>,
}

impl reqwest::dns::Resolve for OwnedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        #[cfg(test)]
        let config = self.config.clone();
        #[cfg(not(test))]
        let config = None;
        Box::pin(async move {
            let mut builder = if let Some(config) = config {
                TokioResolver::builder_with_config(
                    config,
                    hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
                )
            } else {
                TokioResolver::builder_tokio()?
            };
            builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
            // Fixed public GitHub/provider origins use DNS directly. Loading an
            // arbitrarily large hosts file here is synchronous and uncancellable.
            builder.options_mut().use_hosts_file = ResolveHosts::Never;
            // Do not cache a resolver across calls: each transfer owns a new
            // runtime, and a cached resolver could retain its stopped handle.
            let addresses = builder.build()?.lookup_ip(name.as_str()).await?;
            let result: reqwest::dns::Addrs = Box::new(
                addresses
                    .into_iter()
                    .map(|ip| std::net::SocketAddr::new(ip, 0)),
            );
            Ok(result)
        })
    }
}
