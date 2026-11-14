//! ashd: revealing edge layer surfaces over a fullscreen window.
//!
//! ```kdl
//! edge-reveal {
//!     on                          // off by default
//!     dwell-ms 150                // how long the pointer must rest against the edge
//!     hide-delay-ms 400           // how long after the pointer left the surface it slides out
//!     namespace "^ashd-(rail|bar)$" // optional: only layer surfaces with a matching namespace
//! }
//!
//! animations {
//!     edge-reveal {
//!         spring damping-ratio=1.0 stiffness=800 epsilon=0.0001
//!     }
//! }
//! ```
//!
//! Edge surfaces are top-layer layer-shell surfaces anchored to one edge (and not to the
//! opposite one) with a positive exclusive zone: panels like a bar or a dock.

use knuffel::errors::DecodeError;

use crate::animations::{Animation, Kind, SpringParams};
use crate::utils::{MergeWith, RegexEq};

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeReveal {
    pub on: bool,
    pub dwell_ms: u16,
    pub hide_delay_ms: u16,
    pub namespace: Option<RegexEq>,
}

impl Default for EdgeReveal {
    fn default() -> Self {
        Self {
            on: false,
            dwell_ms: 150,
            hide_delay_ms: 400,
            namespace: None,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct EdgeRevealPart {
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child, unwrap(argument))]
    pub dwell_ms: Option<u16>,
    #[knuffel(child, unwrap(argument))]
    pub hide_delay_ms: Option<u16>,
    #[knuffel(child, unwrap(argument, str))]
    pub namespace: Option<RegexEq>,
}

impl MergeWith<EdgeRevealPart> for EdgeReveal {
    fn merge_with(&mut self, part: &EdgeRevealPart) {
        self.on |= part.on;
        if part.off {
            self.on = false;
        }
        merge_clone!((self, part), dwell_ms, hide_delay_ms);
        if part.namespace.is_some() {
            self.namespace.clone_from(&part.namespace);
        }
    }
}

/// Animation of edge surfaces sliding in and out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeRevealAnim(pub Animation);

impl Default for EdgeRevealAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 800,
                epsilon: 0.0001,
            }),
        })
    }
}

impl<S> knuffel::Decode<S> for EdgeRevealAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn parse_edge_reveal() {
        let config = Config::parse_mem("").unwrap();
        assert!(!config.edge_reveal.on);
        assert_eq!(config.edge_reveal.dwell_ms, 150);

        let config = Config::parse_mem(
            r#"
            edge-reveal {
                on
                dwell-ms 200
                hide-delay-ms 250
                namespace "^ashd-(rail|bar)$"
            }
            animations {
                edge-reveal {
                    duration-ms 180
                    curve "ease-out-cubic"
                }
            }
            "#,
        )
        .unwrap();
        assert!(config.edge_reveal.on);
        assert_eq!(config.edge_reveal.dwell_ms, 200);
        assert_eq!(config.edge_reveal.hide_delay_ms, 250);
        let ns = config.edge_reveal.namespace.as_ref().unwrap();
        assert!(ns.0.is_match("ashd-rail"));
        assert!(!ns.0.is_match("ashd-scratchpad"));
        assert!(!config.animations.edge_reveal.0.off);
    }
}
