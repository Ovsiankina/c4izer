//! Tests of the deployment-view row (SRS 4.7: P3-1, P3-2,
//! P3-100..P3-131). One `#[test]` per requirement, named after it. The
//! fixture is `tests/data/deployment_view.dsl`. The editor test (P3-131)
//! is in `crates/editor/src/deployment_view_tests.rs`.
//!
//! Where a test says "Structurizr 6.2.4", the expectation was checked by
//! exporting the same DSL with the installed Structurizr
//! (`~/.local/share/structurizr`).

use std::fmt::Write as _;
use std::sync::atomic::AtomicBool;

use c4rizer_core::lint::Rule;
use c4rizer_core::model::{self, ElementId, ElementKind, Model, ModelError, ModelErrorKind, ViewKind};
use c4rizer_core::pipeline::{self, ViewBuild};
use c4rizer_core::place::Keep;

/// The row's fixture.
const FIXTURE: &str = include_str!("../../../tests/data/deployment_view.dsl");

/// `model_body` and `views_body` inside one workspace.
fn ws(model_body: &str, views_body: &str) -> String {
    format!("workspace {{\n    model {{\n{model_body}\n    }}\n    views {{\n{views_body}\n    }}\n}}\n")
}

fn parse(src: &str) -> Model {
    match model::parse(src) {
        Ok(p) => p.model,
        Err(e) => panic!("{e:?}\n{src}"),
    }
}

fn error(src: &str) -> ModelError {
    match model::parse(src) {
        Ok(_) => panic!("parsed:\n{src}"),
        Err(e) => e,
    }
}

fn build(m: &Model, key: &str) -> ViewBuild {
    match pipeline::build_view(m, key, &Keep::default(), &AtomicBool::new(false)) {
        Ok(b) => b,
        Err(e) => panic!("{key}: {e:?}"),
    }
}

/// The element `key` names.
fn id(m: &Model, key: &str) -> ElementId {
    m.find(key).unwrap_or_else(|| panic!("no element {key}"))
}

/// The view-local key of `e` (`Model::full_key`).
fn key(m: &Model, e: ElementId) -> String {
    m.full_key(e).unwrap_or_default()
}

/// The keys of the boxes of `b`, in box order.
fn boxes(m: &Model, b: &ViewBuild) -> Vec<String> {
    b.plan.boxes.iter().map(|x| key(m, x.element)).collect()
}

/// `src->dst` per drawn line, in edge order.
fn lines(m: &Model, b: &ViewBuild) -> Vec<String> {
    b.plan
        .edges
        .iter()
        .map(|e| {
            let s = b.plan.boxes[e.source.0 as usize].element;
            let d = b.plan.boxes[e.destination.0 as usize].element;
            format!("{}->{}", key(m, s), key(m, d))
        })
        .collect()
}

/// The lint codes of `b`.
fn codes(b: &ViewBuild) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = b.report.findings.iter().map(|f| f.rule.code()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `src->dst` of every copied relationship, in model order.
fn copied(m: &Model) -> Vec<String> {
    m.relationships
        .iter()
        .filter(|r| r.copied_from.is_some())
        .map(|r| format!("{}->{}", key(m, r.source.value), key(m, r.destination.value)))
        .collect()
}

/// The keys `Model::included` gives for view `view` of `m`.
fn included(m: &Model, view: &str) -> Vec<String> {
    m.included(m.view(view).expect("view")).iter().map(|&e| key(m, e)).collect()
}

/// Two systems with instances in one environment, for the `include *`
/// probes (Structurizr 6.2.4).
fn probe(views: &str) -> Model {
    parse(&ws(
        &format!(
            "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                ai = containerInstance a\n                bi = containerInstance b\n                si = softwareSystemInstance shop\n                oi = softwareSystemInstance other\n                lb = infrastructureNode \"LB\"\n                inner = deploymentNode \"Inner\" {{\n                    fw = infrastructureNode \"FW\" \"\" \"iptables\"\n                }}\n            }}\n            m = deploymentNode \"M\" {{\n                ri = softwareSystemInstance other\n            }}\n        }}\n        test = deploymentEnvironment \"Test\" {{\n            t = deploymentNode \"T\" {{\n                ti = containerInstance a\n            }}\n        }}"
        ),
        views,
    ))
}

/// The scene boundary of deployment node `k` in build `b`.
fn node_boundary<'b>(m: &Model, b: &'b ViewBuild, k: &str) -> &'b c4rizer_core::scene::SceneBoundary {
    let e = id(m, k);
    b.scene
        .boundaries
        .iter()
        .find(|x| x.of == c4rizer_core::view::BoundaryOf::DeploymentNode(e))
        .unwrap_or_else(|| panic!("no boundary {k}"))
}

/// The scene box drawing element `k` in build `b`.
fn scene_box<'b>(m: &Model, b: &'b ViewBuild, k: &str) -> &'b c4rizer_core::scene::SceneBox {
    let e = id(m, k);
    b.scene.boxes.iter().find(|x| x.element == e).unwrap_or_else(|| panic!("no box {k}"))
}

/// How many times the polyline `points` crosses the outline of `r`
/// (px), sampled every 5px.
fn crossings_of(points: &[c4rizer_core::geom::Point], r: &c4rizer_core::geom::Rect) -> usize {
    let inside = |x: i32, y: i32| x > r.min.x && x < r.max.x && y > r.min.y && y < r.max.y;
    let mut samples = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = ((b.x - a.x).abs().max((b.y - a.y).abs()) / 5).max(1);
        for i in 0..=n {
            samples.push(inside(a.x + (b.x - a.x) * i / n, a.y + (b.y - a.y) * i / n));
        }
    }
    samples.windows(2).filter(|w| w[0] != w[1]).count()
}

/// Per line of `b`, per deployment node boundary: (boxes of the line it
/// holds, times the line crosses it).
fn line_node_crossings(b: &ViewBuild) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for l in &b.scene.lines {
        for bd in &b.scene.boundaries {
            let held = usize::from(bd.members.contains(&l.source)) + usize::from(bd.members.contains(&l.destination));
            out.push((held, crossings_of(&l.points, &bd.rect)));
        }
    }
    out
}

/// A small shop: roles only.
const SHOP: &str = r#"        user = person "Operator"
        shop = softwareSystem "Shop" {
            a = container "A" "Front." "Rust"
            b = container "B" "Store." "SQL"
        }
        other = softwareSystem "Other"
        user -> a "Uses"
        a -> b "Reads" "SQL"
        a -> other "Calls" "HTTPS""#;


/// Deployment nodes, infrastructure nodes and instances are elements of four new kinds that know their parent node and environment; a static `include *` selects none.
#[test]
fn p3_1_deployment_model_knows_parent_and_environment() {
    let m = parse(FIXTURE);
    let live = m.environments.iter().find(|e| e.name.value == "Live").expect("Live").id;
    let web_host = id(&m, "webHost");
    let cloud = id(&m, "cloud");
    for (k, kind, parent) in [
        ("cloud", ElementKind::DeploymentNode, None),
        ("webHost", ElementKind::DeploymentNode, Some(cloud)),
        ("dns", ElementKind::InfrastructureNode, Some(cloud)),
        ("webBlue", ElementKind::ContainerInstance, Some(web_host)),
        ("mailInstance", ElementKind::SoftwareSystemInstance, Some(id(&m, "provider"))),
    ] {
        let e = m.element(id(&m, k));
        assert_eq!((e.kind, e.parent, e.environment), (kind, parent, Some(live)), "{k}");
    }
    // A static view's `include *` still selects no deployment element
    // (P2-120).
    let static_view = parse(&format!(
        "{}\n",
        FIXTURE.replace(
            "    views {\n",
            "    views {\n        container shop \"c2\" {\n            include *\n        }\n",
        )
    ));
    let c2 = static_view.view("c2").expect("c2");
    let kinds: Vec<ElementKind> = static_view.included(c2).iter().map(|&e| static_view.element(e).kind).collect();
    assert_eq!(kinds, [ElementKind::Person, ElementKind::Container, ElementKind::Container, ElementKind::Container, ElementKind::SoftwareSystem]);
}

/// Each `deploymentEnvironment` is one `Environment` in declaration order with its name and id; a second one with the same name is `DuplicateName`.
#[test]
fn p3_100_environments_in_order_duplicate_name_error() {
    let m = parse(FIXTURE);
    let envs: Vec<(&str, Option<&str>)> = m
        .environments
        .iter()
        .map(|e| (e.name.value.as_str(), e.key.as_ref().map(|k| k.value.as_str())))
        .collect();
    assert_eq!(envs, [("Live", Some("live")), ("Staging", Some("staging"))]);
    assert!(m.environments.iter().enumerate().all(|(i, e)| e.id.0 as usize == i));
    let anonymous = parse(&ws("        deploymentEnvironment \"Test\" {\n        }", ""));
    assert_eq!(anonymous.environments[0].key, None);
    let e = error(&ws(
        "        deploymentEnvironment \"Live\" {\n        }\n        deploymentEnvironment \"Live\" {\n        }",
        "",
    ));
    assert!(matches!(&e.kind, ModelErrorKind::DuplicateName { name, .. } if name == "Live"), "{e:?}");
    assert_eq!(e.position.line, 5);
}

/// A `deploymentNode` holds name, description, technology, tags, `instances`, parent node and environment; a nested node's parent is the enclosing node.
#[test]
fn p3_101_deployment_node_fields_and_nesting() {
    let src = ws(
        &format!(
            "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            outer = deploymentNode \"Outer\" \"Holds things.\" \"AWS\" \"Cloud,Paid\" 3 {{\n                inner = deploymentNode \"Inner\" {{\n                    description \"Inside.\"\n                    technology \"EC2\"\n                    instances 2..5\n                    tags \"Box\"\n                    url https://example.com/inner\n                    properties {{\n                        zone a\n                    }}\n                    containerInstance a\n                }}\n            }}\n        }}"
        ),
        "",
    );
    let m = parse(&src);
    let outer = m.element(id(&m, "outer"));
    assert_eq!(outer.name.value, "Outer");
    assert_eq!(outer.description.as_ref().map(|d| d.value.as_str()), Some("Holds things."));
    assert_eq!(outer.technology.as_ref().map(|t| t.value.as_str()), Some("AWS"));
    assert_eq!(outer.tags, ["Element", "Deployment Node", "Cloud", "Paid"]);
    assert_eq!(outer.instances.as_ref().map(|i| i.value.as_str()), Some("3"));
    assert_eq!(outer.parent, None);
    let inner = m.element(id(&m, "inner"));
    assert_eq!(inner.parent, Some(outer.id));
    assert_eq!(inner.environment, outer.environment);
    assert_eq!(inner.description.as_ref().map(|d| d.value.as_str()), Some("Inside."));
    assert_eq!(inner.technology.as_ref().map(|t| t.value.as_str()), Some("EC2"));
    assert_eq!(inner.instances.as_ref().map(|i| i.value.as_str()), Some("2..5"));
    assert_eq!(inner.tags, ["Element", "Deployment Node", "Box"]);
    assert_eq!(inner.url.as_ref().map(|u| u.value.as_str()), Some("https://example.com/inner"));
    assert_eq!(inner.properties.len(), 1);
    // Unwritten `instances` stays `None` (counts as 1).
    let fixture = parse(FIXTURE);
    assert_eq!(fixture.element(id(&fixture, "database")).instances, None);
}

/// `instances` takes a positive integer or `m..n` with `m <= n`, kept as written (default 1); anything else is an error naming it.
#[test]
fn p3_102_instances_value_checked() {
    let node = |value: &str| {
        ws(
            &format!("        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                instances {value}\n            }}\n        }}"),
            "",
        )
    };
    for ok in ["1", "7", "1..3", "2..2"] {
        let m = parse(&node(ok));
        assert_eq!(m.element(id(&m, "n")).instances.as_ref().map(|i| i.value.as_str()), Some(ok));
    }
    for bad in ["0", "3..1", "x", "1..", "0..2", "-1"] {
        let e = error(&node(bad));
        assert!(
            matches!(&e.kind, ModelErrorKind::InvalidValue { construct, value } if construct == "instances" && value == bad),
            "{bad}: {e:?}"
        );
    }
    // The positional form is checked the same way.
    let e = error(&ws(
        "        live = deploymentEnvironment \"Live\" {\n            n = deploymentNode \"N\" \"\" \"\" \"\" \"many\"\n        }",
        "",
    ));
    assert!(matches!(&e.kind, ModelErrorKind::InvalidValue { value, .. } if value == "many"), "{e:?}");
}

/// An `infrastructureNode` holds name, description, technology, tags and parent node.
#[test]
fn p3_103_infrastructure_node_fields() {
    let m = parse(FIXTURE);
    let lb = m.element(id(&m, "lb"));
    assert_eq!(lb.kind, ElementKind::InfrastructureNode);
    assert_eq!(lb.name.value, "Load balancer");
    assert_eq!(lb.description.as_ref().map(|d| d.value.as_str()), Some("Spreads requests."));
    assert_eq!(lb.technology.as_ref().map(|t| t.value.as_str()), Some("ELB"));
    assert_eq!(lb.tags, ["Element", "Infrastructure Node"]);
    assert_eq!(lb.parent, Some(id(&m, "cloud")));
    let src = ws(
        "        live = deploymentEnvironment \"Live\" {\n            n = deploymentNode \"N\" {\n                fw = infrastructureNode \"Firewall\" {\n                    technology \"iptables\"\n                    tags \"Edge\"\n                }\n            }\n        }",
        "",
    );
    let m = parse(&src);
    let fw = m.element(id(&m, "fw"));
    assert_eq!(fw.technology.as_ref().map(|t| t.value.as_str()), Some("iptables"));
    assert_eq!(fw.tags, ["Element", "Infrastructure Node", "Edge"]);
}

/// `softwareSystemInstance`, `containerInstance` and `instanceOf` give an instance with parent, groups and a number per element per environment.
#[test]
fn p3_104_instances_numbered_per_element_per_environment() {
    let m = parse(FIXTURE);
    let web = id(&m, "web");
    let blue = m.element(id(&m, "webBlue"));
    let green = m.element(id(&m, "webGreen"));
    assert_eq!((blue.instance_of, blue.instance_number), (Some(web), Some(1)));
    assert_eq!((green.instance_of, green.instance_number), (Some(web), Some(2)));
    assert_eq!(blue.deployment_groups, ["Blue"]);
    assert_eq!(m.element(id(&m, "dbPrimary")).deployment_groups, ["Blue", "Green"]);
    assert_eq!(blue.parent, Some(id(&m, "webHost")));
    // Staging numbers its own instances from 1 again.
    let staging = m.environments[1].id;
    let staged: Vec<(String, Option<u32>)> = m
        .elements
        .iter()
        .filter(|e| e.environment == Some(staging) && e.instance_of.is_some())
        .map(|e| (key(&m, e.id), e.instance_number))
        .collect();
    assert_eq!(staged, [("web_1".to_owned(), Some(1)), ("api_1".to_owned(), Some(1)), ("db_1".to_owned(), Some(1))]);
    // `instanceOf` takes a system or a container; the kind must fit.
    let inst = |line: &str| {
        ws(
            &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                {line}\n            }}\n        }}"),
            "",
        )
    };
    let m = parse(&inst("x = instanceOf shop\n                y = instanceOf a"));
    assert_eq!(m.element(id(&m, "x")).kind, ElementKind::SoftwareSystemInstance);
    assert_eq!(m.element(id(&m, "y")).kind, ElementKind::ContainerInstance);
    assert!(matches!(error(&inst("containerInstance shop")).kind, ModelErrorKind::InvalidValue { .. }));
    assert!(matches!(error(&inst("softwareSystemInstance a")).kind, ModelErrorKind::InvalidValue { .. }));
    // An unknown deployment group is ignored with a no-effect notice, as
    // Structurizr does (corpus big-bank-plc.dsl: `containerInstance
    // database "Failover"`).
    let m = parse(&inst("x = containerInstance a \"nogroup\""));
    assert!(m.element(id(&m, "x")).deployment_groups.is_empty());
    assert!(m.notices.iter().any(|n| n.subject.contains("nogroup")), "{:?}", m.notices);
}

/// An instance carries its kind tag and own tags, after the tags of the element it instantiates.
#[test]
fn p3_105_instance_tags_include_element_tags() {
    let m = parse(FIXTURE);
    assert_eq!(
        m.element(id(&m, "dbPrimary")).tags,
        ["Element", "Container", "Database", "Container Instance"]
    );
    assert_eq!(
        m.element(id(&m, "mailInstance")).tags,
        ["Element", "Software System", "External", "Software System Instance"]
    );
    let src = ws(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                x = containerInstance a \"\" \"Hot\" {{\n                    tag \"Pinned\"\n                }}\n            }}\n        }}"),
        "        styles {\n            element \"Container Instance\" {\n                background #112233\n            }\n        }",
    );
    let m = parse(&src);
    let x = id(&m, "x");
    assert_eq!(m.element(x).tags, ["Element", "Container", "Container Instance", "Hot", "Pinned"]);
    assert_eq!(m.style_of(x).background.to_string(), "#112233");
}

/// A model relationship is copied between each pair of instances of its ends that share a deployment group (default group included).
#[test]
fn p3_106_copied_relationships_share_a_group() {
    // The probed case (Structurizr 6.2.4): A_1, B_1 in group g1, A_2, B_2
    // in g2 give A_1 -> B_1 and A_2 -> B_2, not A_1 -> B_2.
    let src = ws(
        &format!(
            "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            g1 = deploymentGroup \"G1\"\n            g2 = deploymentGroup \"G2\"\n            n = deploymentNode \"N\" {{\n                containerInstance a \"g1\"\n                containerInstance b \"g1\"\n                containerInstance a \"g2\"\n                containerInstance b \"g2\"\n            }}\n        }}"
        ),
        "",
    );
    let m = parse(&src);
    assert_eq!(copied(&m), ["a_1->b_1", "a_2->b_2"]);
    let r = m.relationships.iter().find(|r| r.copied_from.is_some()).unwrap();
    let original = m.relationship(r.copied_from.unwrap());
    assert_eq!(r.description, original.description);
    assert_eq!(r.technology, original.technology);
    assert_eq!(r.tags, original.tags);
    // Ungrouped instances share the default group; other environments
    // never mix.
    let src = ws(
        &format!(
            "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                containerInstance a\n                containerInstance b\n                softwareSystemInstance other\n            }}\n        }}\n        test = deploymentEnvironment \"Test\" {{\n            t = deploymentNode \"T\" {{\n                containerInstance b\n            }}\n        }}"
        ),
        "",
    );
    let m = parse(&src);
    assert_eq!(copied(&m), ["a_1->b_1", "a_1->other_1"]);
    // The fixture: blue with blue, green with green, the shared database
    // with both; `apiGreen -/> mailInstance` removed one.
    let m = parse(FIXTURE);
    assert_eq!(
        copied(&m),
        [
            "webBlue->apiBlue",
            "webGreen->apiGreen",
            "web_1->api_1",
            "apiBlue->dbPrimary",
            "apiGreen->dbPrimary",
            "api_1->db_1",
            "apiBlue->mailInstance",
        ]
    );
}

/// A deployment relationship from a node or infrastructure node is kept; one between two instances is "not permitted".
#[test]
fn p3_107_instance_to_instance_relationship_is_error() {
    let env = |lines: &str| {
        ws(
            &format!(
                "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                ai = containerInstance a\n                bi = containerInstance b\n                lb = infrastructureNode \"LB\"\n            }}\n{lines}\n        }}"
            ),
            "",
        )
    };
    let m = parse(&env("            lb -> ai \"Forwards\"\n            n -> lb \"Hosts\"\n            ai -> lb \"Logs to\""));
    let written: Vec<String> = m
        .relationships
        .iter()
        .filter(|r| r.copied_from.is_none() && m.element(r.source.value).environment.is_some())
        .map(|r| format!("{}->{}", key(&m, r.source.value), key(&m, r.destination.value)))
        .collect();
    assert_eq!(written, ["lb->ai", "n->lb", "ai->lb"]);
    let e = error(&env("            ai -> bi \"Calls\""));
    assert_eq!(
        e.kind,
        ModelErrorKind::DeploymentRelationship {
            source: "ai".into(),
            destination: "bi".into(),
            reason: "not permitted"
        }
    );
    assert!(e.to_string().contains("not permitted"), "{e}");
}

/// `a -/> b` removes the copied relationship between instances `a` and `b`; none to remove is an error naming both.
#[test]
fn p3_108_removal_drops_one_copied_relationship() {
    let env = |lines: &str| {
        ws(
            &format!(
                "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                ai = containerInstance a\n                bi = containerInstance b\n                oi = softwareSystemInstance other\n                lb = infrastructureNode \"LB\"\n            }}\n{lines}\n        }}"
            ),
            "",
        )
    };
    let m = parse(&env(""));
    assert_eq!(copied(&m), ["ai->bi", "ai->oi"]);
    let m = parse(&env("            ai -/> bi"));
    assert_eq!(copied(&m), ["ai->oi"]);
    // A description selects the relationship whose copy goes.
    let m = parse(&env("            ai -/> oi \"Calls\""));
    assert_eq!(copied(&m), ["ai->bi"]);
    for (line, src, dst) in [("            bi -/> ai", "bi", "ai"), ("            ai -/> oi \"Other\"", "ai", "oi")] {
        let e = error(&env(line));
        assert_eq!(
            e.kind,
            ModelErrorKind::DeploymentRelationship {
                source: src.into(),
                destination: dst.into(),
                reason: "no copied relationship"
            },
            "{line}"
        );
    }
    // Both ends must be instances of that environment.
    assert!(matches!(error(&env("            lb -/> ai")).kind, ModelErrorKind::UndeclaredId { .. }));
}

/// `a -/> b` naming a container stands for all its instances in the environment, and a `{ }` block after it holds `->` lines that take the removed relationship's description and technology unless written (Structurizr 6.2.4, corpus no-relationship.dsl).
#[test]
fn p3_108_removal_block_and_container_ends() {
    let src = ws(
        &format!(
            "{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                ai = containerInstance a\n                bi = containerInstance b\n                lb = infrastructureNode \"LB\"\n            }}\n            a -/> b {{\n                a -> lb\n                lb -> b \"Forwards\" \"\"\n            }}\n        }}"
        ),
        "",
    );
    let m = parse(&src);
    assert!(copied(&m).iter().all(|c| c != "ai->bi"), "{:?}", copied(&m));
    let written: Vec<(String, Option<&str>, Option<&str>)> = m
        .relationships
        .iter()
        .filter(|r| r.copied_from.is_none() && m.element(r.source.value).environment.is_some())
        .map(|r| {
            (
                format!("{}->{}", key(&m, r.source.value), key(&m, r.destination.value)),
                r.description.as_ref().map(|d| d.value.as_str()),
                r.technology.as_ref().map(|t| t.value.as_str()),
            )
        })
        .collect();
    assert_eq!(
        written,
        [
            ("ai->lb".to_owned(), Some("Reads"), Some("SQL")),
            ("lb->bi".to_owned(), Some("Forwards"), Some("")),
        ]
    );
    // Only `->` lines go in the block.
    let bad = src.replace("                a -> lb\n", "                tags \"X\"\n");
    assert!(matches!(error(&bad).kind, ModelErrorKind::UnknownConstruct { .. }), "{:?}", error(&bad));
}

/// Under `!identifiers hierarchical` a deployment id is its full dotted path from the environment; a bare id resolves inside its block.
#[test]
fn p3_109_hierarchical_deployment_ids() {
    let src = ws(
        "        !identifiers hierarchical\n        shop = softwareSystem \"Shop\" {\n            a = container \"A\"\n        }\n        live = deploymentEnvironment \"Live\" {\n            aws = deploymentNode \"AWS\" {\n                ec2 = deploymentNode \"EC2\" {\n                    webi = containerInstance shop.a\n                    lb = infrastructureNode \"LB\"\n                    lb -> webi \"Forwards\"\n                }\n            }\n            live.aws.ec2.lb -> live.aws.ec2.webi \"Again\"\n        }",
        "",
    );
    let m = parse(&src);
    let webi = id(&m, "live.aws.ec2.webi");
    assert_eq!(m.element(webi).kind, ElementKind::ContainerInstance);
    assert_eq!(key(&m, webi), "live.aws.ec2.webi");
    assert_eq!(m.find("webi"), None);
    let lb = id(&m, "live.aws.ec2.lb");
    let written: Vec<_> = m
        .relationships
        .iter()
        .filter(|r| r.source.value == lb)
        .map(|r| r.destination.value)
        .collect();
    assert_eq!(written, [webi, webi]);
}

/// A deployment view is drawable, leaves `unsupported`, draws nested node boundaries with their instances and gives each instance a key.
#[test]
fn p3_2_deployment_view_is_drawn() {
    use c4rizer_core::view::BoundaryOf;
    assert!(ViewKind::Deployment.drawable());
    let m = parse(FIXTURE);
    let all = pipeline::build_all(&m, &AtomicBool::new(false)).expect("build");
    assert!(all.unsupported.is_empty());
    let keys: Vec<&str> = all.views.iter().map(|v| v.plan.key.as_str()).collect();
    assert_eq!(keys, ["live", "staging", "live-apps"]);
    let b = build(&m, "live");
    assert_eq!(
        boxes(&m, &b),
        ["dns", "lb", "webBlue", "webGreen", "apiBlue", "apiGreen", "dbPrimary", "mailInstance"]
    );
    let nodes: Vec<String> = b
        .plan
        .boundaries
        .iter()
        .map(|x| match x.of {
            BoundaryOf::DeploymentNode(e) => key(&m, e),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(nodes, ["cloud", "webHost", "apiHost", "database", "provider"]);
    // Nested: the cloud's members hold every host's members.
    let cloud = &b.plan.boundaries[0].members;
    assert!(b.plan.boundaries[1..4].iter().all(|x| x.members.iter().all(|m| cloud.contains(m))));
}

/// A view scoped to `S`: `include *` selects the container instances of `S`, every software system instance and infrastructure node, no person.
#[test]
fn p3_110_scoped_include_star() {
    let m = probe("        deployment shop \"Live\" \"v\" {\n            include *\n        }");
    // No person, no container instance of another system, nothing of Test.
    assert_eq!(included(&m, "v"), ["ai", "bi", "si", "oi", "lb", "fw", "ri"]);
}

/// `deployment *`: every container instance, infrastructure node, and software system instance of a system with no container instance shown.
#[test]
fn p3_111_wildcard_include_star() {
    let m = probe("        deployment * \"Live\" \"v\" {\n            include *\n        }");
    // `si` is left out: its system has container instances in the view.
    assert_eq!(included(&m, "v"), ["ai", "bi", "oi", "lb", "fw", "ri"]);
    let m = probe("        deployment * \"Test\" \"t\" {\n            include *\n        }");
    assert_eq!(included(&m, "t"), ["ti"]);
}

/// `include`/`exclude` take node, infrastructure node and instance ids (a node brings everything under it) and deployment `element.type` expressions.
#[test]
fn p3_112_include_node_ids_and_expressions() {
    let m = probe(
        "        deployment * \"Live\" \"nodes\" {\n            include n\n            exclude inner\n        }\n        deployment * \"Live\" \"ids\" {\n            include lb ri\n        }\n        deployment * \"Live\" \"types\" {\n            include \"element.type==InfrastructureNode\" \"element.type==SoftwareSystemInstance\"\n        }\n        deployment * \"Live\" \"tech\" {\n            include *\n            exclude \"element.technology==iptables\"\n        }\n        deployment * \"Live\" \"node-type\" {\n            include \"element.type==DeploymentNode\"\n        }",
    );
    assert_eq!(included(&m, "nodes"), ["ai", "bi", "si", "oi", "lb"]);
    assert_eq!(included(&m, "ids"), ["lb", "ri"]);
    assert_eq!(included(&m, "types"), ["si", "oi", "lb", "fw", "ri"]);
    assert_eq!(included(&m, "tech"), ["ai", "bi", "oi", "lb", "ri"]);
    // Selecting the nodes selects everything under them, in Live only.
    assert_eq!(included(&m, "node-type"), ["ai", "bi", "si", "oi", "lb", "fw", "ri"]);
    // A tag expression selects instances by their element's tags too.
    let m = parse(FIXTURE.replace("include *\n            title \"Shop in production\"", "include \"element.tag==Database\"\n            title \"Shop in production\"").as_str());
    assert_eq!(included(&m, "live"), ["dbPrimary"]);
}

/// A node with no drawn instance or infrastructure node under it is not drawn.
#[test]
fn p3_113_empty_node_not_drawn() {
    use c4rizer_core::view::BoundaryOf;
    let m = parse(FIXTURE);
    let spare = id(&m, "spare");
    let b = build(&m, "live");
    assert!(!b.plan.boundaries.iter().any(|x| x.of == BoundaryOf::DeploymentNode(spare)));
    // A node whose only content is excluded is not drawn either.
    let src = FIXTURE.replace("include *\n            title \"Shop in production\"", "include *\n            exclude mailInstance\n            title \"Shop in production\"");
    let m = parse(&src);
    let provider = id(&m, "provider");
    let b = build(&m, "live");
    assert!(!b.plan.boundaries.iter().any(|x| x.of == BoundaryOf::DeploymentNode(provider)));
}

/// A node boundary is solid, tagged at the top left `name` / `[Deployment Node: tech]`, and nests at least 20px inside its parent.
#[test]
fn p3_114_node_boundary_solid_tag_top_left_nested() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    let cloud = node_boundary(&m, &b, "cloud");
    assert_eq!(cloud.stroke.dash_px, None, "solid");
    assert_eq!(cloud.tag.lines, ["Cloud", "[Deployment Node: AWS]"]);
    assert_eq!(cloud.tag_rect.min, cloud.rect.min, "top left");
    assert!(cloud.tag.fits);
    for inner in ["webHost", "apiHost", "database"] {
        let r = node_boundary(&m, &b, inner).rect;
        assert!(
            r.min.x - cloud.rect.min.x >= 20
                && r.min.y - cloud.rect.min.y >= 20
                && cloud.rect.max.x - r.max.x >= 20
                && cloud.rect.max.y - r.max.y >= 20,
            "{inner} {r:?} in {:?}",
            cloud.rect
        );
    }
    // No technology: `[Deployment Node]`; a styled border wins.
    let src = ws(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"Plain\" {{\n                containerInstance a\n            }}\n        }}"),
        "        deployment * \"Live\" \"v\" \"V\" {\n            include *\n        }\n        styles {\n            element \"Deployment Node\" {\n                border dashed\n            }\n        }",
    );
    let m = parse(&src);
    let b = build(&m, "v");
    let n = node_boundary(&m, &b, "n");
    assert_eq!(n.tag.lines, ["Plain", "[Deployment Node]"]);
    assert!(n.stroke.dash_px.is_some());
}

/// A node whose `instances` is not 1 shows `x<value>` at its bottom-right corner in the node's text colour.
#[test]
fn p3_115_instance_count_bottom_right() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    for (k, text) in [("webHost", "x2"), ("apiHost", "x2..4")] {
        let n = node_boundary(&m, &b, k);
        let count = n.instances.as_ref().unwrap_or_else(|| panic!("{k}"));
        assert_eq!(count.lines, [text]);
        assert_eq!(count.colour, n.tag.colour);
        let r = n.rect;
        assert!(count.bounds.max.x <= r.max.x && count.bounds.max.y <= r.max.y);
        assert!(count.bounds.min.x > r.centre().x && count.bounds.min.y > r.centre().y, "bottom right");
        assert!(c4rizer_core::export::svg(&b.scene).contains(&format!(">{text}<")));
    }
    assert!(node_boundary(&m, &b, "database").instances.is_none());
    assert!(node_boundary(&m, &b, "cloud").instances.is_none());
}

/// Instance boxes show `[Container: tech]` / `[Software System]`; infrastructure nodes `[Infrastructure Node: tech]`.
#[test]
fn p3_116_instance_type_lines() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    let web = scene_box(&m, &b, "webBlue");
    assert_eq!(web.name.lines, ["Web"]);
    assert_eq!(web.type_line.lines, ["[Container: Rust]"]);
    assert_eq!(web.description.as_ref().map(|d| d.lines.clone()), Some(vec!["Serves the pages.".to_owned()]));
    assert_eq!(scene_box(&m, &b, "mailInstance").type_line.lines, ["[External Software System]"]);
    assert_eq!(scene_box(&m, &b, "lb").type_line.lines, ["[Infrastructure Node: ELB]"]);
    let src = ws(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" {{\n                si = softwareSystemInstance shop\n            }}\n        }}"),
        "        deployment * \"Live\" \"v\" \"V\" {\n            include *\n        }",
    );
    let m = parse(&src);
    assert_eq!(scene_box(&m, &build(&m, "v"), "si").type_line.lines, ["[Software System]"]);
}

/// An instance's key is its written id, else `<element key>_<n>`; two instances of one element are two boxes with two keys.
#[test]
fn p3_117_view_local_keys() {
    let m = parse(FIXTURE);
    // Written ids where there are some, `<element key>_<n>` otherwise.
    assert_eq!(included(&m, "staging"), ["web_1", "api_1", "db_1"]);
    let b = build(&m, "live");
    let blue = b.plan.boxes_of(id(&m, "webBlue"));
    let green = b.plan.boxes_of(id(&m, "webGreen"));
    assert_eq!((blue.len(), green.len()), (1, 1));
    assert_ne!(blue, green);
    assert_eq!(m.element(id(&m, "webBlue")).instance_of, m.element(id(&m, "webGreen")).instance_of);
    // `at` lines name an instance without an id by its key, within the
    // view's environment.
    let src = FIXTURE.replace("include *\n            title \"Shop in staging\"", "include *\n            at api_1 400 300\n            title \"Shop in staging\"");
    let m = parse(&src);
    let v = m.view("staging").unwrap();
    assert_eq!(key(&m, v.positions[0].element.value), "api_1");
    assert_eq!(m.element(v.positions[0].element.value).environment, Some(m.environments[1].id));
    // An edit moves it by that key (DSL-9).
    let parsed = model::parse(FIXTURE).expect("fixture");
    let applied = c4rizer_core::edit::apply(
        &parsed,
        &[c4rizer_core::edit::Edit::Move {
            view: "staging".into(),
            element: "db_1".into(),
            centre: Some((600, 800)),
        }],
    )
    .expect("move");
    assert!(applied.text.contains("            at db_1 600 800\n"), "{}", applied.text);
}

/// Copied and written deployment relationships between drawn boxes are drawn; a model relationship never is.
#[test]
fn p3_118_only_instance_lines_drawn() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    assert_eq!(
        lines(&m, &b),
        [
            "dns->lb",
            "lb->webBlue",
            "lb->webGreen",
            "webBlue->apiBlue",
            "webGreen->apiGreen",
            "apiBlue->dbPrimary",
            "apiGreen->dbPrimary",
            "apiBlue->mailInstance",
        ]
    );
    // Every line draws a copied or a written deployment relationship.
    for e in &b.plan.edges {
        let r = m.relationship(e.winner());
        assert!(r.copied_from.is_some() || m.element(r.source.value).environment.is_some());
    }
    assert_eq!(b.plan.edges[3].description.as_deref(), Some("Sends orders to"));
}

/// The title is the `title` line else the description; the subtitle line is drawn; neither gives L8.
#[test]
fn p3_119_title_and_subtitle() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    assert_eq!(b.plan.title.as_deref(), Some("Shop in production"));
    assert_eq!(b.plan.subtitle.as_deref(), Some("Blue and green tiers"));
    assert!(!codes(&b).contains(&"L8"));
    let m = probe("        deployment * \"Live\" \"d\" \"Everything live\" {\n            include *\n        }\n        deployment * \"Live\" \"none\" {\n            include *\n        }");
    assert_eq!(build(&m, "d").plan.title.as_deref(), Some("Everything live"));
    assert!(codes(&build(&m, "none")).contains(&"L8"));
}

/// A keyless deployment view gets `Deployment-NNN`.
#[test]
fn p3_120_generated_deployment_key() {
    let m = probe("        deployment * \"Live\" {\n            include *\n        }\n        deployment shop \"Test\" {\n            include *\n        }");
    let keys: Vec<&str> = m.views.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(keys, ["Deployment-001", "Deployment-002"]);
    assert_eq!(m.views[0].kind, ViewKind::Deployment);
}

/// Auto-placement puts a node's children inside it; sibling boundaries keep 60px apart; two runs are equal.
#[test]
fn p3_121_nodes_placed_as_blocks() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    // Every box sits inside each of its nodes' boundaries.
    for bd in &b.scene.boundaries {
        for x in &b.scene.boxes {
            let r = x.rect;
            let inside = r.min.x >= bd.rect.min.x && r.max.x <= bd.rect.max.x && r.min.y >= bd.rect.min.y && r.max.y <= bd.rect.max.y;
            assert_eq!(inside, bd.members.contains(&x.id), "{:?} / {:?}", x.name.lines, bd.tag.lines);
        }
    }
    // Siblings keep at least 60px apart.
    let gap = |a: &str, c: &str| {
        let (p, q) = (node_boundary(&m, &b, a).rect, node_boundary(&m, &b, c).rect);
        let dx = (q.min.x - p.max.x).max(p.min.x - q.max.x);
        let dy = (q.min.y - p.max.y).max(p.min.y - q.max.y);
        dx.max(dy)
    };
    for (a, c) in [("webHost", "apiHost"), ("apiHost", "database"), ("webHost", "database"), ("cloud", "provider")] {
        assert!(gap(a, c) >= 60, "{a} {c}: {}", gap(a, c));
    }
    // CORE-25: two runs, equal centres.
    assert_eq!(build(&m, "live").routes.centres, b.routes.centres);
}

/// `at`/`size` take instance and infrastructure node ids; naming a deployment node is `DeploymentNodeLayout`.
#[test]
fn p3_122_at_on_node_is_error() {
    let view = |line: &str| probe(&format!("        deployment * \"Live\" \"v\" {{\n            include *\n            {line}\n        }}"));
    let m = view("at ai 200 200\n            size lb 300 160");
    let v = m.view("v").unwrap();
    assert_eq!(v.positions.len(), 1);
    assert_eq!(v.sizes.len(), 1);
    for line in ["at n 100 100", "size inner 400 300"] {
        let src = ws(
            "        live = deploymentEnvironment \"Live\" {\n            n = deploymentNode \"N\" {\n                inner = deploymentNode \"Inner\" {\n                    lb = infrastructureNode \"LB\"\n                }\n            }\n        }",
            &format!("        deployment * \"Live\" \"v\" {{\n            include *\n            {line}\n        }}"),
        );
        let e = error(&src);
        let id = line.split(' ').nth(1).unwrap();
        assert_eq!(e.kind, ModelErrorKind::DeploymentNodeLayout { id: id.into() }, "{line}");
        assert!(e.to_string().contains("boundary follows its contents"), "{e}");
    }
}

/// A line crosses a node boundary only to reach an end inside it; a line inside one node stays inside it.
#[test]
fn p3_123_lines_cross_only_needed_boundaries() {
    let m = parse(FIXTURE);
    for view in ["live", "staging", "live-apps"] {
        let b = build(&m, view);
        for (held, crossed) in line_node_crossings(&b) {
            // A node holding one end is crossed exactly once; one holding
            // both or neither is never crossed.
            assert_eq!(crossed, usize::from(held == 1), "{view}: held {held}, crossed {crossed}");
        }
    }
}

/// The fixture's view `live` has no L1, L3, L24 and crossings within `maxCrossings`.
#[test]
fn p3_124_layout_targets_on_fixture() {
    let m = parse(FIXTURE);
    let b = build(&m, "live");
    let c = codes(&b);
    for rule in ["L1", "L3", "L4", "L24"] {
        assert!(!c.contains(&rule), "{rule}: {:?}", b.report.findings);
    }
    assert!(b.routes.crossings <= b.plan.options.max_crossings);
}

/// A deployment view runs L1..L14 and L23..L26 and none of L15..L21.
#[test]
fn p3_125_deployment_lint_rule_set() {
    // A view built to trip the kind rules: a technology on every line, a
    // protocol word, more than 8 boxes, components nowhere.
    let mut nodes = String::new();
    for i in 0..10 {
        let _ = writeln!(nodes, "                i{i} = infrastructureNode \"HTTP node {i}\" \"Node.\" \"Tech\"");
    }
    let src = ws(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" \"Host.\" \"EC2\" {{\n{nodes}                ai = containerInstance a\n            }}\n            i0 -> ai \"Forwards requests over HTTPS to\" \"HTTPS\"\n        }}"),
        "        deployment * \"Live\" \"v\" \"V\" {\n            include *\n        }",
    );
    let m = parse(&src);
    let b = build(&m, "v");
    let c = codes(&b);
    for rule in ["L15", "L16", "L17", "L18", "L19", "L20", "L21"] {
        assert!(!c.contains(&rule), "{rule}: {c:?}");
    }
    // The generic rules still run: L7 (alignment) and L9 here.
    let src = src.replace("\"HTTP node 3\" \"Node.\"", "\"HTTP node 3\" \"\"");
    let c = codes(&build(&parse(&src), "v"));
    assert!(c.contains(&"L9"), "{c:?}");
    assert_eq!(Rule::ForeignNodeCrossing.code(), "L27");
}

/// L27 (error): a line crosses a node boundary holding neither of its ends.
#[test]
fn p3_126_l27_line_through_foreign_node() {
    use c4rizer_core::geom::Point;
    use c4rizer_core::lint::Severity;
    let src = ws(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            top = deploymentNode \"Top\" \"T.\" \"EC2\" {{\n                ai = containerInstance a\n            }}\n            mid = deploymentNode \"Mid\" \"M.\" \"EC2\" {{\n                lb = infrastructureNode \"LB\" \"L.\" \"ELB\"\n            }}\n            low = deploymentNode \"Low\" \"L.\" \"EC2\" {{\n                bi = containerInstance b\n            }}\n        }}"),
        "        deployment * \"Live\" \"v\" \"V\" {\n            include *\n            at ai 300 200\n            at lb 300 600\n            at bi 300 1000\n        }",
    );
    let m = parse(&src);
    let mut b = build(&m, "v");
    // The router skirts node `Mid` (P3-123): no L27.
    assert_eq!(lines(&m, &b), ["ai->bi"]);
    assert!(!codes(&b).contains(&"L27"), "{:?}", b.report.findings);
    // A line drawn straight down through `Mid` is L27, naming the node.
    b.scene.lines[0].points = vec![Point::new(300, 270), Point::new(300, 930)];
    let report = c4rizer_core::lint::lint(&m, &b.scene);
    let l27: Vec<_> = report.findings.iter().filter(|f| f.rule == Rule::ForeignNodeCrossing).collect();
    assert_eq!(l27.len(), 1, "{:?}", report.findings);
    assert!(l27[0].message.contains("\"Mid\""), "{}", l27[0].message);
    assert_eq!(Rule::ForeignNodeCrossing.severity(), Severity::Error);
    // Crossing a node holding an end is no L27.
    assert!(!l27[0].message.contains("\"Top\""));
    let fixture = parse(FIXTURE);
    assert!(!codes(&build(&fixture, "live")).contains(&"L27"));
}

/// L10 on a container instance reads its container's technology.
#[test]
fn p3_127_l10_reads_container_technology() {
    let view = "        deployment * \"Live\" \"v\" \"V\" {\n            include *\n        }";
    let env = |shop: &str| {
        ws(
            &format!("{shop}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\" \"Host.\" \"EC2\" {{\n                ai = containerInstance a\n            }}\n        }}"),
            view,
        )
    };
    let m = parse(&env(SHOP));
    assert!(!codes(&build(&m, "v")).contains(&"L10"));
    let m = parse(&env(&SHOP.replace("container \"A\" \"Front.\" \"Rust\"", "container \"A\" \"Front.\"")));
    let b = build(&m, "v");
    let l10: Vec<_> = b.report.findings.iter().filter(|f| f.rule == Rule::NoTechnology).collect();
    assert_eq!(l10.len(), 1);
    assert!(l10[0].message.contains("\"A\""));
}

/// The five deployment inspections give one violation per node naming it.
#[test]
fn p3_128_deployment_inspections() {
    use c4rizer_core::inspect::{Inspection, Subject, inspect};
    let m = parse(FIXTURE);
    let src = FIXTURE.replace(
        "spare = deploymentNode \"Spare\" \"Holds nothing yet.\" \"EC2\"",
        "spare = deploymentNode \"Spare\"",
    )
    .replace("infrastructureNode \"DNS\" \"Routes requests.\" \"Route 53\"", "infrastructureNode \"DNS\"");
    let bare = parse(&src);
    let deployment = |m: &Model| -> Vec<(Inspection, String)> {
        inspect(m)
            .into_iter()
            .filter(|v| v.inspection.key().contains("deploymentnode") || v.inspection.key().contains("infrastructurenode"))
            .map(|v| match v.subject {
                Subject::Element(e) => (v.inspection, key(m, e)),
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(deployment(&m), [(Inspection::DeploymentNodeEmpty, "spare".to_owned())]);
    assert_eq!(
        deployment(&bare),
        [
            (Inspection::InfrastructureNodeDescription, "dns".to_owned()),
            (Inspection::InfrastructureNodeTechnology, "dns".to_owned()),
            (Inspection::DeploymentNodeDescription, "spare".to_owned()),
            (Inspection::DeploymentNodeTechnology, "spare".to_owned()),
            (Inspection::DeploymentNodeEmpty, "spare".to_owned()),
        ]
    );
    let keys: Vec<&str> = [
        Inspection::DeploymentNodeDescription,
        Inspection::DeploymentNodeTechnology,
        Inspection::DeploymentNodeEmpty,
        Inspection::InfrastructureNodeDescription,
        Inspection::InfrastructureNodeTechnology,
    ]
    .iter()
    .map(|i| i.key())
    .collect();
    assert_eq!(
        keys,
        [
            "model.deploymentnode.description",
            "model.deploymentnode.technology",
            "model.deploymentnode.empty",
            "model.infrastructurenode.description",
            "model.infrastructurenode.technology",
        ]
    );
    // No deployment element is `model.element.disconnected` (Structurizr
    // 6.2.4); only the empty node is not in any view.
    let others: Vec<(Inspection, String)> = inspect(&m)
        .into_iter()
        .filter_map(|v| match v.subject {
            Subject::Element(e) if m.element(e).environment.is_some() => Some((v.inspection, key(&m, e))),
            _ => None,
        })
        .filter(|(i, _)| matches!(i, Inspection::ElementDisconnected | Inspection::ElementNoView))
        .collect();
    assert_eq!(others, [(Inspection::ElementNoView, "spare".to_owned())]);
}

/// `views.view.empty` counts what a deployment view's `include *` selects.
#[test]
fn p3_129_view_empty_counts_deployment_include() {
    use c4rizer_core::inspect::{Inspection, inspect};
    let m = parse(FIXTURE);
    assert!(!inspect(&m).iter().any(|v| v.inspection == Inspection::ViewEmpty));
    let empty = parse(&ws(
        &format!("{SHOP}\n        live = deploymentEnvironment \"Live\" {{\n            n = deploymentNode \"N\"\n        }}"),
        "        deployment * \"Live\" \"v\" \"V\" {\n            include *\n        }",
    ));
    assert!(inspect(&empty).iter().any(|v| v.inspection == Inspection::ViewEmpty));
}

/// A filtered view on a deployment base draws it with the tag filter.
#[test]
fn p3_130_filtered_view_on_deployment_base() {
    let m = parse(FIXTURE);
    let b = build(&m, "live-apps");
    assert_eq!(boxes(&m, &b), ["webBlue", "webGreen", "apiBlue", "apiGreen", "dbPrimary", "mailInstance"]);
    assert!(!lines(&m, &b).iter().any(|l| l.starts_with("lb") || l.starts_with("dns")));
    assert_eq!(b.plan.title.as_deref(), Some("Production without infrastructure"));
    // The boundaries of the base view stay around the kept boxes.
    assert_eq!(b.plan.boundaries.len(), 5);
}

/// P3-124: building the fixture's view `live` takes under 1000 ms in release.
#[test]
#[ignore = "perf: run with --release -- --ignored perf_"]
fn perf_p3_124() {
    let m = parse(FIXTURE);
    let start = std::time::Instant::now();
    let b = build(&m, "live");
    let took = start.elapsed();
    assert_eq!(b.plan.boxes.len(), 8);
    assert!(took.as_millis() < 1000, "{took:?}");
}

/// A relationship written between components is lifted to their containers
/// (implied relationship, P2-127) and copied between the containers'
/// instances; a container pair that already has a relationship of its own
/// is not given a second copy (Structurizr 6.2.4, default implied strategy).
#[test]
fn polish_04_component_relationships_are_copied_to_container_instances() {
    let src = ws(
        "        s = softwareSystem \"S\" {\n            c1 = container \"C1\" {\n                k1 = component \"K1\"\n                k3 = component \"K3\"\n            }\n            c2 = container \"C2\" {\n                k2 = component \"K2\"\n            }\n            c3 = container \"C3\"\n        }\n        k1 -> k2 \"Calls\"\n        k3 -> k2 \"Also calls\"\n        c1 -> c3 \"Direct\"\n        k1 -> c3 \"Component to container\"\n        live = deploymentEnvironment \"Live\" {\n            n = deploymentNode \"N\" {\n                containerInstance c1\n                containerInstance c2\n                containerInstance c3\n            }\n        }",
        "",
    );
    let m = parse(&src);
    assert_eq!(copied(&m), ["c1_1->c2_1", "c1_1->c3_1"]);
}
