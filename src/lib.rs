use tokio::stream::StreamExt;
use tokio::time::interval;
use std::iter::Iterator;
use std::collections::{HashSet, HashMap, VecDeque};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use bcrypt::{DEFAULT_COST, hash, verify};
use warp::{Filter, http::StatusCode, sse::ServerSentEvent};
use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};
use std::convert::Infallible;
use std::time::Duration;
use lazy_static::lazy_static;
use bus::Bus;
use crossbeam::channel::unbounded;
use inflector::Inflector;
use json_patch::merge;
use std::sync::{Arc, Mutex};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use base64::{decode as base64_decode};

// init database as lazy
lazy_static! {
    static ref TREE: HashMap<String, sled::Db> = {
        let configure = config();
        let tree = sled::open(configure.save_path).unwrap();
        let mut m = HashMap::new();
        m.insert("tree".to_owned(), tree);
        m
    };
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Token {
    pub jwt: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Record {
    pub event: Event,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserCollection {
    pub info: Vec<Event>,
    pub events: Vec<Event>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Collection {
    pub events: Vec<Event>,
}

#[derive(Debug, Clone)]
pub struct SSE {
    pub event: String,
    pub data: String,
    pub id: String,
    pub retry: Duration,
    pub tenant_id: uuid::Uuid,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
  pub port: u16,
  pub expiry: i64,
  pub origin: String,
  pub secret: String,
  pub save_path: String,
  pub connection: String,
  pub cert_path: String,
  pub key_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JWT {
    check: bool,
    claims: Claims,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    id: uuid::Uuid,
    username: String,
    password: String,
    collection_id: uuid::Uuid,
    tenant_id: uuid::Uuid,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserForm {
    username: String,
    password: String,
    collection_id: uuid::Uuid,
    tenant_id: uuid::Uuid,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Login {
    username: String,
    password: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    sub: String,
    company: String,
    exp: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Cfg {
  pub save_path: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Event {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub collection_id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub event: String,
    pub timestamp: i64,
    pub published: bool,
    pub cancelled: bool,
    pub data: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EventForm {
    collection_id: uuid::Uuid,
    tenant_id: uuid::Uuid,
    event: String,
    timestamp: i64,
    data: serde_json::Value,
}

// helper function to create sse events
fn get_events(tenant_id: uuid::Uuid) -> Vec<SSE> {
    let tree = TREE.get(&"tree".to_owned()).unwrap();
    let mut vals : Vec<Event> = tree.iter().into_iter().filter(|x| {
        let p = x.as_ref().unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        if k.contains("_v_") {
            let v = std::str::from_utf8(&p.1).unwrap().to_owned();
            let evt : Event = serde_json::from_str(&v).unwrap();
            if !evt.cancelled && evt.tenant_id == tenant_id {
                return true
            } else {
                return false
            }
        } else {
            return false
        }
    }).map(|x| {
        let p = x.as_ref().unwrap();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        let evt : Event = serde_json::from_str(&v).unwrap();
        evt
    }).collect();

    vals.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    let mut uniques : HashSet<String> = HashSet::new();
    for evt in &vals {
        uniques.insert(evt.clone().event);
    }

    let mut sse_events : Vec<SSE> = Vec::new();

    for evt in uniques {
        let mut events : HashMap<String, Event> = HashMap::new();
        for event in &vals {
            if evt == event.event {
                events.insert(event.clone().collection_id.to_string(), event.clone());
            }
        }
        let mut evts : Vec<Event> = Vec::new();
        let mut uniq_data_keys : HashSet<String> = HashSet::new();
        let mut rows : Vec<serde_json::Value> = Vec::new();
        for (_, v) in events {
            if v.clone().data.is_object() {
                evts.push(v.clone());
                let mut data = v.clone().data;
                let j = json!({"timestamp": v.clone().timestamp.to_string()});
                merge(&mut data, &j);
                let j = json!({"collection_id": v.clone().collection_id});
                merge(&mut data, &j);
                rows.push(data);
                for (k, _) in v.clone().data.as_object().unwrap() {
                    uniq_data_keys.insert(k.clone());
                }
            }
        }

        rows.sort_by(|a, b| a.get("timestamp").unwrap().to_string().cmp(&b.get("timestamp").unwrap().to_string()));
        rows.reverse();

        let mut columns : VecDeque<serde_json::Value> = VecDeque::new();
        for uniq_key in uniq_data_keys {
            if uniq_key != "collection_id" && uniq_key != "timestamp" {
                columns.push_back(json!({"title": Inflector::to_sentence_case(&uniq_key), "field": uniq_key}));
            }
        }

        let mut cols : Vec<&serde_json::Value> = columns.iter().collect();
        cols.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
        let mut colz : VecDeque<serde_json::Value> = VecDeque::new();
        for col in cols {
            colz.push_back(col.clone());
        }
        colz.push_front(json!({"title": "collection_id", "field": "collection_id"}));
        colz.push_front(json!({"title": "Timestamp", "field": "timestamp"}));

        let guid = Uuid::new_v4().to_string();
        let events_json = json!({"events": evts, "columns": colz, "rows": rows});
        sse_events.push(SSE{id: guid, event: evt, data: serde_json::to_string(&events_json).unwrap(), retry: Duration::from_millis(5000), tenant_id: tenant_id});
    }
    sse_events
}

// get ntp time from global servers (cloudflare primary and fallback pool)
pub fn get_ntp_time() -> i64 {
    let pool_ntp = "pool.ntp.org:123";
    let cf_ntp = "time.cloudflare.com:123";
    let response = match broker_ntp::request(cf_ntp) {
        Ok(res) => res,
        Err(_) => broker_ntp::request(pool_ntp).unwrap()
    };
    let timestamp = response.transmit_timestamp;
    broker_ntp::unix_time::Instant::from(timestamp).secs()
}

// cancel future event
fn cancel(tree: sled::Db, event_id: String, user_id: String) -> String {

    let versioned = format!("_u_{}", user_id);
    let g = tree.get(&versioned.as_bytes()).unwrap().unwrap();
    let v = std::str::from_utf8(&g).unwrap().to_owned();
    let user : User = serde_json::from_str(&v).unwrap();

    let versioned = format!("_v_{}", event_id);
    let g = tree.get(&versioned.as_bytes()).unwrap().unwrap();
    let v = std::str::from_utf8(&g).unwrap().to_owned();
    let mut json : Event = serde_json::from_str(&v).unwrap();
    let j = json.clone();
    if json.tenant_id == user.tenant_id {
        json.cancelled = true;
        let _ = tree.compare_and_swap(versioned.as_bytes(), Some(serde_json::to_string(&j).unwrap().as_bytes()), Some(serde_json::to_string(&json).unwrap().as_bytes()));
        let _ = tree.flush();
    }
    json!({"event": json}).to_string()
}

// display user collection of events
fn user_collection(tree: sled::Db, id: String) -> String {

    let versioned = format!("_u_{}", id);
    let g = tree.get(&versioned.as_bytes()).unwrap().unwrap();
    let v = std::str::from_utf8(&g).unwrap().to_owned();
    let user : User = serde_json::from_str(&v).unwrap();

    // turn iVec(s) to String(s) and make HashMap
    let mut info: Vec<Event> = tree.iter().into_iter().filter(|x| {
        let p = x.as_ref().unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        if k.contains(&"_v_") {
            let v = std::str::from_utf8(&p.1).unwrap().to_owned();
            let j : Event = serde_json::from_str(&v).unwrap();
            if j.collection_id.to_string() == user.collection_id.to_string() {
                return true
            } else {
                return false
            }
        } else {
            return false
        }
    }).map(|x| {
        let p = x.unwrap();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        let j : Event = serde_json::from_str(&v).unwrap();
        j
    }).collect();

    info.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    // turn iVec(s) to String(s) and make HashMap
    let mut owned: Vec<Event> = tree.iter().into_iter().filter(|x| {
        let p = x.as_ref().unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        if k.contains(&"_v_") {
            let v = std::str::from_utf8(&p.1).unwrap().to_owned();
            let j : Event = serde_json::from_str(&v).unwrap();
            if j.user_id.to_string() == user.id.to_string() {
                return true
            } else {
                return false
            }
        } else {
            return false
        }
    }).map(|x| {
        let p = x.unwrap();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        let j : Event = serde_json::from_str(&v).unwrap();
        j
    }).collect();

    owned.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    let c = UserCollection{info: info, events: owned};
    let data : String = serde_json::to_string(&c).unwrap();
    data
}

// display collection of events based on collection_id
fn collection(tree: sled::Db, collection_id: String, user_id: String) -> String {
 
    let versioned = format!("_u_{}", user_id);
    let g = tree.get(&versioned.as_bytes()).unwrap().unwrap();
    let v = std::str::from_utf8(&g).unwrap().to_owned();
    let user : User = serde_json::from_str(&v).unwrap();

    let mut records: Vec<Event> = tree.iter().into_iter().filter(|x| {
        let p = x.as_ref().unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        if k.contains(&"_v_") {
            let v = std::str::from_utf8(&p.1).unwrap().to_owned();
            let j : Event = serde_json::from_str(&v).unwrap();
            if j.collection_id.to_string() == collection_id && j.tenant_id == user.tenant_id {
                return true
            } else {
                return false
            }
        } else {
            return false
        }
    }).map(|x| {
        let p = x.unwrap();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        let j : Event = serde_json::from_str(&v).unwrap();
        j
    }).collect();

    records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    let c = Collection{events: records};
    let data : String = serde_json::to_string(&c).unwrap();
    data
}

// create a user
fn user_create(tree: sled::Db, user_form: UserForm) -> (bool, String) {
 
    let records : HashMap<String, String> = tree.iter().into_iter().filter(|x| {
        let p = x.as_ref().unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        if k.contains("_u_") {
            let user : User = serde_json::from_str(&v).unwrap();
            if user.username == user_form.username {
                return true
            } else {
                return false
            }
        }
        return false
    }).map(|x| {
        let p = x.unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        (k, v)
    }).collect();

    if records.len() > 0 {
        let j = json!({"error": "username already taken"}).to_string();
        return (false, j)
    } else {
        // set as future value
        let uuid = Uuid::new_v4();
        let versioned = format!("_u_{}", uuid.to_string());
        let hashed = hash(user_form.clone().password, DEFAULT_COST).unwrap();
        let new_user = User{id: uuid, username: user_form.clone().username, password: hashed, collection_id: user_form.clone().collection_id, tenant_id: user_form.clone().tenant_id };
        
        let _ = tree.compare_and_swap(versioned.as_bytes(), None as Option<&[u8]>, Some(serde_json::to_string(&new_user).unwrap().as_bytes())); 
        let _ = tree.flush();
        let j = json!({"id": uuid.to_string()}).to_string();
        return (true, j)
    }
}

// login with user creds
fn login(tree: sled::Db, login: Login, config: Config) -> (bool, String) {

    let now = get_ntp_time();
    let expi = now + config.expiry;
    let expiry = expi as usize;

    let records : HashMap<String, String> = tree.iter().into_iter().filter(|x| {
        let p = x.as_ref().unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        if k.contains(&"_u_") {
            let v = std::str::from_utf8(&p.1).unwrap().to_owned();
            let user : User = serde_json::from_str(&v).unwrap();
            if user.username == login.username {
                return true
            } else {
                return false
            }
        } else {
            return false
        }
    }).map(|x| {
        let p = x.unwrap();
        let k = std::str::from_utf8(&p.0).unwrap().to_owned();
        let v = std::str::from_utf8(&p.1).unwrap().to_owned();
        (k, v)
    }).collect();

    for (_k, v) in records {
        let user : User = serde_json::from_str(&v).unwrap();
        let verified = verify(login.password, &user.password).unwrap();
        if verified {
            let my_claims = Claims{company: "".to_owned(), sub: user.id.to_string(), exp: expiry};
            let token = encode(&Header::default(), &my_claims, &EncodingKey::from_secret(config.secret.as_ref())).unwrap();
            let j = json!({"jwt": token}).to_string();
            return (true, j)
        } else {
            return (false, "".to_owned())
        }
    }
    (false, "".to_owned())
}

// config based on sane local dev defaults (uses double dashes for flags)
fn config() -> Config {
 
    let mut port : u16 = 8080;
    let mut expiry : i64 = 3600;
    let mut connection = "http".to_owned();
    let mut origin = "http://localhost:3000".to_owned();
    let mut secret = "secret".to_owned();
    let mut key_path = "./broker.rsa".to_owned();
    let mut cert_path = "./broker.pem".to_owned();
    let _ : Vec<String> = go_flag::parse(|flags| {
        flags.add_flag("port", &mut port);
        flags.add_flag("origin", &mut origin);
        flags.add_flag("expiry", &mut expiry);
        flags.add_flag("secret", &mut secret);
        flags.add_flag("connection", &mut connection);
        flags.add_flag("key-path", &mut key_path);
        flags.add_flag("cert-path", &mut cert_path);
    });

    let save_path = match envy::from_env::<Cfg>() {
        Ok(cfg) => cfg.save_path,
        Err(_) => "./tmp/broker_data".to_owned()
    };

    Config{port: port, secret: secret, origin: origin, save_path: save_path, expiry: expiry, connection: connection, key_path: key_path, cert_path: cert_path}
}

// verify the exp and key of the JWT or the HTTP Basic Username/Password
fn jwt_verify(config: Config, token: String) -> JWT {
  
    let mut parts = token.split(" ");
    let auth_type = parts.next().unwrap();
    if auth_type == "Bearer" {
        let token = parts.next().unwrap();
        let _ = match decode::<Claims>(&token,  &DecodingKey::from_secret(config.secret.as_ref()), &Validation::default()) {
            Ok(c) => {
                return JWT{check: true, claims: c.claims};
            },
            Err(_) => {
                return JWT{check: false, claims: Claims{company: "".to_owned(), exp: 0, sub: "".to_owned()}};
            }
        };
    } else if auth_type == "Basic" {
        let token = parts.next().unwrap();
        let _ = match &base64_decode(token) {
            Ok(c) => {
                let _ = match std::str::from_utf8(&c) {
                    Ok(d) => {
                        let mut username_password = d.split(":");
                        let username = username_password.next().unwrap();
                        let password = username_password.next().unwrap();
                        let tree = TREE.get(&"tree".to_owned()).unwrap();

                        let records : HashMap<String, String> = tree.iter().into_iter().filter(|x| {
                            let p = x.as_ref().unwrap();
                            let k = std::str::from_utf8(&p.0).unwrap().to_owned();
                            if k.contains(&"_u_") {
                                let v = std::str::from_utf8(&p.1).unwrap().to_owned();
                                let user : User = serde_json::from_str(&v).unwrap();
                                if user.username == username {
                                    return true
                                } else {
                                    return false
                                }
                            } else {
                                return false
                            }
                        }).map(|x| {
                            let p = x.unwrap();
                            let k = std::str::from_utf8(&p.0).unwrap().to_owned();
                            let v = std::str::from_utf8(&p.1).unwrap().to_owned();
                            (k, v)
                        }).collect();
                    
                        for (_k, v) in records {
                            let user : User = serde_json::from_str(&v).unwrap();
                            let verified = verify(password, &user.password).unwrap();
                            if verified {
                                return JWT{check: true, claims: Claims{company: "".to_owned(), exp: 0, sub: user.id.to_string()}};
                            } else {
                                return JWT{check: false, claims: Claims{company: "".to_owned(), exp: 0, sub: "".to_owned()}};
                            }
                        }
                    },
                    Err(_) => {
                        return JWT{check: false, claims: Claims{company: "".to_owned(), exp: 0, sub: "".to_owned()}};
                    }
                };
            },
            Err(_) => {
                return JWT{check: false, claims: Claims{company: "".to_owned(), exp: 0, sub: "".to_owned()}};
            }
        };
    }
    JWT{check: false, claims: Claims{company: "".to_owned(), exp: 0, sub: "".to_owned()}}
}

// insert an event
fn insert(tree: sled::Db, user_id: String, evt: EventForm) -> String {
  
    // get user
    let versioned = format!("_u_{}", user_id);
    let g = tree.get(&versioned.as_bytes()).unwrap().unwrap();
    let v = std::str::from_utf8(&g).unwrap().to_owned();
    let user : User = serde_json::from_str(&v).unwrap();

    // build event object
    let id = Uuid::new_v4();
    let j = Event{id: id, published: false, cancelled: false, data: evt.data, event: evt.event, timestamp: evt.timestamp, user_id: user.id, collection_id: evt.collection_id, tenant_id: evt.tenant_id};
    let new_value_string = serde_json::to_string(&j).unwrap();
    let new_value = new_value_string.as_bytes();
    let versioned = format!("_v_{}", id.to_string());

    // only write if form tenant_id and user tenant_id
    if user.tenant_id == evt.tenant_id {
        let _ = tree.compare_and_swap(versioned, None as Option<&[u8]>, Some(new_value.clone())); 
        let _ = tree.flush();
        return json!({"event": j}).to_string()
    }

    json!({"error": "trying to write to wrong tenant"}).to_string()
}

// create a sse event
fn event_stream(rx: crossbeam::channel::Receiver<SSE>, allowed: bool) -> Result<impl ServerSentEvent, Infallible> {

    if allowed {
        let sse = match rx.try_recv() {
            Ok(sse) => sse,
            Err(_) => {
                let id = Uuid::new_v4();
                let guid = id.to_string();
                let polling = json!({"status": "polling"});
                SSE{id: guid, event: "internal_status".to_owned(), data: polling.to_string(), retry: Duration::from_millis(5000), tenant_id: id}
            }
        };
        Ok((
            warp::sse::id(sse.id),
            warp::sse::data(sse.data),
            warp::sse::event(sse.event),
            warp::sse::retry(sse.retry),
        ))
    } else {
        let id = Uuid::new_v4();
        let guid = id.to_string();
        let denied = json!({"error": "denied"});
        let sse = SSE{id: guid, event: "internal_status".to_owned(), data: denied.to_string(), retry: Duration::from_millis(5000), tenant_id: id};
        Ok((
            warp::sse::id(sse.id),
            warp::sse::data(sse.data),
            warp::sse::event(sse.event),
            warp::sse::retry(sse.retry),
        ))
    }
}

// main function
pub async fn broker() {

    // start logging
    pretty_env_logger::init();

    // user create route
    let user_create_route = warp::post()
        .and(warp::path("users"))
        .and(warp::body::json())
        .map(move |user: UserForm| {
            let tree = TREE.get(&"tree".to_owned()).unwrap();
            let (check, value) = user_create(tree.clone(), user.clone());
            if check {
                let reply = warp::reply::with_status(value, StatusCode::OK);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            } else {
                let reply = warp::reply::with_status(value, StatusCode::BAD_REQUEST);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            }
        });
    
    // auth check middleware
    let auth_check = warp::header::<String>("authorization").map(|token| {
        let configure = config();
        jwt_verify(configure, token)
    });

    // login route
    let login_route = warp::post()
        .and(warp::path("login"))
        .and(warp::body::json())
        .map(move |login_form: Login| {
            let configure = config();
            let tree = TREE.get(&"tree".to_owned()).unwrap();
            let (check, value) = login(tree.clone(), login_form.clone(), configure.clone());
            if check {
                let reply = warp::reply::with_status(value, StatusCode::OK);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            } else {
                let reply = warp::reply::with_status(value, StatusCode::UNAUTHORIZED);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            }
        });

    // insert route
    let insert_route = warp::post()
        .and(warp::path("insert"))
        .and(auth_check)
        .and(warp::body::json())
        .map(move |jwt: JWT, event_form: EventForm| {
            if jwt.check {
                let tree = TREE.get(&"tree".to_owned()).unwrap();
                let record = insert(tree.clone(), jwt.claims.sub, event_form);
                let reply = warp::reply::with_status(record, StatusCode::OK);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            } else {
                let reply = warp::reply::with_status("".to_owned(), StatusCode::UNAUTHORIZED);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            }
        });

    // create thread-safe broadcast bus
    let mix_tx = Bus::new(100);
    let tx = Arc::new(Mutex::new(mix_tx));
    let tx2 = tx.clone();

    // create tokio worker thread that will dispatch events to bus
    let _ = tokio::spawn(async move {
        loop {
            // get events that have not been published or cancelled
            let tree = TREE.get(&"tree".to_owned()).unwrap();
            let vals : HashMap<String, Event> = tree.iter().into_iter().filter(|x| {
                let p = x.as_ref().unwrap();
                let k = std::str::from_utf8(&p.0).unwrap().to_owned();
                if k.contains("_v_") {
                    let v = std::str::from_utf8(&p.1).unwrap().to_owned();
                    let evt : Event = serde_json::from_str(&v).unwrap();
                    if !evt.published && !evt.cancelled {
                        let now = get_ntp_time();
                        if evt.timestamp <= now  {
                            return true
                        } else {
                            return false
                        }
                    } else {
                        return false
                    }
                } else {
                    return false
                }
            }).map(|x| {
                let p = x.as_ref().unwrap();
                let k = std::str::from_utf8(&p.0).unwrap().to_owned();
                let v = std::str::from_utf8(&p.1).unwrap().to_owned();
                let evt : Event = serde_json::from_str(&v).unwrap();
                let evt_cloned = evt.clone();
                (k, evt_cloned)
            }).collect();

            // publish these filtered events to bus
            for (k, v) in vals {
                let old_json = v.clone();
                let old_json_clone = old_json.clone();
                let mut new_json = v.clone();
                new_json.published = true;
                let newest_json = new_json.clone();
                let newer_json = newest_json.clone();
                let tree_cloned = tree.clone();

                let _ = tokio::spawn(async move {
                    let _ = tree_cloned.compare_and_swap(k, Some(serde_json::to_string(&old_json_clone).unwrap().as_bytes()), Some(serde_json::to_string(&newest_json).unwrap().as_bytes())); 
                    let _ = tree_cloned.flush();
                }).await;
                
                tx2.lock().unwrap().broadcast(newer_json);
            }
        }  
    });
    
    // create bus middleware
    let with_sender = warp::any().map(move || tx.clone());

    // sse route
    let sse_route = warp::path("events")
        .and(auth_check)
        .and(with_sender)
        .and(warp::path::param::<uuid::Uuid>())
        .and(warp::get()).map(move |jwt: JWT, tx_main: Arc<Mutex<bus::Bus<Event>>>, tenant_id: uuid::Uuid| {

        // create recv for bus (each sse instance must have its own)
        let mut rx_main = tx_main.lock().unwrap().add_rx();

        // create local sse channel
        let (tx, rx) = unbounded();

        // loop through sse events to send on load of sse route
        for event in get_events(tenant_id) {
            let _ = tx.send(event);
        }

        // every 100ms check the bus and if any messages send to local channel also check local channel and publish to stream (sse route)
        let event_stream = interval(Duration::from_millis(100)).map(move |_| {
            let evt = match rx_main.try_recv() {
                Ok(evt) => {
                    if tenant_id == evt.tenant_id {
                        evt
                    } else {
                        let id = Uuid::new_v4();
                        Event{id: id, published: false, cancelled: false, data: json!({"test": "test"}), event: "fake".to_owned(), timestamp: 123, user_id: id, collection_id: id, tenant_id: id}
                    }
                },
                Err(_) => {
                    let id = Uuid::new_v4();
                    Event{id: id, published: false, cancelled: false, data: json!({"test": "test"}), event: "fake".to_owned(), timestamp: 123, user_id: id, collection_id: id, tenant_id: id}
                }
            };
            for event in get_events(evt.tenant_id) {
                let _ = tx.send(event);
            }
            event_stream(rx.clone(), jwt.check)
        });
        warp::sse::reply(event_stream)
    });

    // cancel route
    let cancel_route = warp::get()
        .and(warp::path("cancel"))
        .and(auth_check)
        .and(warp::path::param::<String>())
        .map(move |jwt: JWT, event_id: String| {
            if jwt.check {
                let tree = TREE.get(&"tree".to_owned()).unwrap();
                let record = cancel(tree.clone(), event_id, jwt.claims.sub);
                let reply = warp::reply::with_status(record, StatusCode::OK);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            } else {
                let reply = warp::reply::with_status("".to_owned(), StatusCode::UNAUTHORIZED);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            }
        });

    // collections route
    let collections_route = warp::get()
        .and(warp::path("collections"))
        .and(auth_check)
        .and(warp::path::param::<String>())
        .map(move |jwt: JWT, collection_id: String| {
            if jwt.check {
                let tree = TREE.get(&"tree".to_owned()).unwrap();
                let record = collection(tree.clone(), collection_id, jwt.claims.sub);
                let reply = warp::reply::with_status(record, StatusCode::OK);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            } else {
                let reply = warp::reply::with_status("".to_owned(), StatusCode::UNAUTHORIZED);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            }
        });

    // user collection route
    let user_collection_route = warp::get()
        .and(warp::path("user_events"))
        .and(auth_check)
        .map(move |jwt: JWT| {
            if jwt.check {
                let tree = TREE.get(&"tree".to_owned()).unwrap();
                let record = user_collection(tree.clone(), jwt.claims.sub);
                let reply = warp::reply::with_status(record, StatusCode::OK);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            } else {
                let reply = warp::reply::with_status("".to_owned(), StatusCode::UNAUTHORIZED);
                warp::reply::with_header(reply, "Content-Type", "application/json")
            }
        });

    // create cors wrapper
    let configure = config();
    let mut cors = warp::cors().allow_origin(&*configure.origin).allow_methods(vec!["GET", "POST"]).allow_headers(vec![warp::http::header::AUTHORIZATION, warp::http::header::CONTENT_TYPE]);

    // handle allow any origin case
    if configure.origin == "*" {
        cors = warp::cors().allow_any_origin().allow_methods(vec!["GET", "POST"]).allow_headers(vec![warp::http::header::AUTHORIZATION, warp::http::header::CONTENT_TYPE]);
    }

    // create routes
    let routes = warp::any().and(login_route).or(user_create_route).or(insert_route).or(sse_route).or(cancel_route).or(collections_route).or(user_collection_route).with(cors);

    // set ip and port
    let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), configure.port);

    // start server based on https or http
    if configure.connection == "https" {
        return warp::serve(routes)
            .tls()
            .cert_path(&configure.cert_path)
            .key_path(&configure.key_path)
            .run(socket).await
    } else {
       return warp::serve(routes).run(socket).await
    }
}