use actix_session::Session;
use actix_web::{get, post, rt::spawn, web, Error, HttpResponse};
use argon2::Config;
use lettre::{transport::smtp::authentication::Credentials, Message, SmtpTransport, Transport};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use uuid::Uuid;
use yarte::{auto, ywrite_min, Template};

use crate::models::{Confirmation, User};
use crate::templates::HomeTemplate;
use crate::{database, vars};

type DBPool = Pool<SqliteConnectionManager>;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(home)
        .service(login)
        .service(login_post)
        .service(logout)
        .service(register)
        .service(register_post)
        .service(confirm);
}

#[get("/")]
async fn home(session: Session) -> Result<HttpResponse, Error> {
    if let Some(auth) = session.get::<String>("auth")? {
        match serde_json::from_str::<User>(&auth) {
            Ok(user) => {
                return Ok(HttpResponse::Ok().body(
                    HomeTemplate::valid(user.email().clone(), user.password().clone())
                        .call()
                        .unwrap(),
                ));
            }
            Err(_) => {
                return Ok(HttpResponse::Ok().body("Invalid authentication"));
            }
        }
    }

    Ok(HttpResponse::Ok().body(HomeTemplate::invalid().call().unwrap()))
}

#[get("/login")]
async fn login(session: Session) -> Result<HttpResponse, Error> {
    if let Some(auth) = session.get::<String>("auth")? {
        match serde_json::from_str::<User>(&auth) {
            Ok(_) => {
                return Ok(HttpResponse::Found().header("LOCATION", "/").finish());
            }
            Err(_) => {
                return Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> invalid }}"))));
            }
        }
    }

    Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> login }}"))))
}

#[post("/login")]
async fn login_post(
    session: Session,
    pool: web::Data<DBPool>,
    user: web::Form<User>,
) -> Result<HttpResponse, Error> {
    if let Some(password) = encode(user.password()) {
        let conn = pool.get().expect("Couldn't get connection from pool");
        let email = user.email().clone();

        if let Some(user) = web::block(move || database::get_user(&conn, &email))
            .await
            .expect("Database error while getting password")
        {
            if &password == user.password() {
                session.set("auth", serde_json::to_string(&user).unwrap())?;
                return Ok(HttpResponse::Found().header("LOCATION", "/").finish());
            }
        }
    }

    Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> invalid }}"))))
}

#[post("/logout")]
async fn logout(session: Session) -> Result<HttpResponse, Error> {
    session.purge();

    Ok(HttpResponse::Found().header("LOCATION", "/").finish())
}

#[get("/register")]
async fn register(session: Session) -> Result<HttpResponse, Error> {
    if let Some(auth) = session.get::<String>("auth")? {
        match serde_json::from_str::<User>(&auth) {
            Ok(_) => {
                return Ok(HttpResponse::Found().header("LOCATION", "/").finish());
            }
            Err(_) => {
                return Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> invalid }}"))));
            }
        }
    }

    Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> register }}"))))
}

#[post("/register")]
async fn register_post(
    pool: web::Data<DBPool>,
    user: web::Form<User>,
) -> Result<HttpResponse, Error> {
    let conn = pool.get().expect("Couldn't get connection from pool");

    let uuid = Uuid::new_v4().to_simple().to_string();

    let confirmation = Confirmation::new(uuid, user.email().clone(), user.password().clone());

    let confirmation_mail = confirmation.clone();

    web::block(move || database::add_confirmation(&conn, &confirmation))
        .await
        .expect("Database error while adding confirmation");

    spawn(send_mail(confirmation_mail));

    Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> mail }}"))))
}

#[get("/confirm/{uuid}")]
async fn confirm(
    session: Session,
    pool: web::Data<DBPool>,
    web::Path(uuid): web::Path<String>,
) -> Result<HttpResponse, Error> {
    let conn = pool.get().expect("Couldn't get connection from pool");
    let uuid_clone = uuid.clone();

    if let Some(confirmation) = web::block(move || database::get_confirmation(&conn, &uuid_clone))
        .await
        .expect("Database error while getting confirmation")
    {
        let conn = pool.get().expect("Couldn't get connection from pool");
        let uuid_clone = uuid.clone();

        web::block(move || database::delete_confirmation(&conn, &uuid_clone))
            .await
            .expect("Database error while deleting confirmation");

        let conn = pool.get().expect("Couldn't get connection from pool");

        if let Some(password) = encode(confirmation.password()) {
            let user = User::new(confirmation.email().clone(), password);
            let user_clone = user.clone();

            web::block(move || database::add_user(&conn, &user_clone))
                .await
                .expect("Database error while getting confirmation");

            session.set("auth", serde_json::to_string(&user).unwrap())?;
            return Ok(HttpResponse::Found().header("LOCATION", "/").finish());
        }
    }

    Ok(HttpResponse::Ok().body(auto!(ywrite_min!(String, "{{> invalid }}"))))
}

async fn send_mail(confirmation: Confirmation) {
    let email = Message::builder()
        .from("Julius Kreutz <jk@julius-kreutz.de>".parse().unwrap())
        .to(format!("<{}>", confirmation.email()).parse().unwrap())
        .subject("Confirmation")
        .body(format!(
            "http://{}:{}/confirm/{}",
            vars::domain(),
            vars::port(),
            confirmation.uuid()
        ))
        .unwrap();

    let creds = Credentials::new(vars::smtp_username(), vars::smtp_password());

    let mailer = SmtpTransport::relay(&vars::smtp_host())
        .unwrap()
        .credentials(creds)
        .build();

    match mailer.send(&email) {
        Ok(_) => println!("Email sent successfully!"),
        Err(e) => panic!("Could not send email: {:?}", e),
    }
}

fn encode(password: &String) -> Option<String> {
    match argon2::hash_encoded(
        password.as_bytes(),
        vars::salt().as_bytes(),
        &Config::default(),
    ) {
        Ok(hash) => Some(hash),
        Err(_) => None,
    }
}