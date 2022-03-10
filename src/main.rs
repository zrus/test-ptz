#![allow(dead_code)]
use std::str::FromStr;

use async_std::task;
use onvif::{schema, soap};
use url::Url;

struct Device {
    pub device_mgmt: soap::client::Client,
    pub media: Option<soap::client::Client>,
    pub ptz: Option<soap::client::Client>,
}

const RELATIVE_BLACKLIST: &str = "IPD-E24Y00";

impl Device {
    pub fn new(url: Option<Url>, usr: Option<String>, pwd: Option<String>) -> Result<Self, String> {
        let creds = match (usr, pwd) {
            (Some(usr), Some(pwd)) => Some(soap::client::Credentials {
                username: usr,
                password: pwd,
            }),
            (None, None) => None,
            _ => panic!("Username and password must be specified together"),
        };

        let base_uri = url.as_ref().ok_or_else(|| "uri must be specified")?;

        let device_mgmt_uri = base_uri.join("onvif/device_service").unwrap();

        let mut out = Self {
            device_mgmt: soap::client::ClientBuilder::new(&device_mgmt_uri)
                .credentials(creds.clone())
                .build(),
            media: None,
            ptz: None,
        };

        let services = task::block_on(schema::devicemgmt::get_services(
            &out.device_mgmt,
            &Default::default(),
        ))
        .unwrap();

        for s in &services.service {
            let url = Url::parse(&s.x_addr).map_err(|e| e.to_string())?;
            if !url.as_str().starts_with(base_uri.as_str()) {
                return Err(format!(
                    "Service URI {} is not within base URI {}",
                    &s.x_addr, &base_uri
                ));
            }

            let svc = Some(
                soap::client::ClientBuilder::new(&url)
                    .credentials(creds.clone())
                    .build(),
            );

            match s.namespace.as_str() {
                "http://www.onvif.org/ver10/device/wsdl" => {
                    if s.x_addr != device_mgmt_uri.as_str() {
                        return Err(format!(
                            "advertised device mgmt uri {} not expected {}",
                            &s.x_addr, &device_mgmt_uri
                        ));
                    }
                }
                "http://www.onvif.org/ver10/media/wsdl" => out.media = svc,
                "http://www.onvif.org/ver20/ptz/wsdl" => out.ptz = svc,
                _ => {}
            }
        }

        Ok(out)
    }
}

async fn get_profile_token(device: &Device) -> schema::onvif::ReferenceToken {
    let media_client = device.media.as_ref().unwrap();
    let profile = &schema::media::get_profiles(media_client, &Default::default())
        .await
        .unwrap()
        .profiles[0];
    schema::onvif::ReferenceToken(profile.token.0.clone())
}

async fn send_continuous_ptz(device: &Device, pan: f64, tilt: f64, zoom: f64) {
    if let Some(ref ptz) = device.ptz {
        let profile_token = get_profile_token(device).await;

        println!("continuous pan: {}, tilt: {}, zoom: {}", pan, tilt, zoom);
        let pan_tilt = Some(schema::common::Vector2D {
            x: pan,
            y: tilt,
            space: None,
        });
        let zoom = Some(schema::common::Vector1D {
            x: zoom,
            space: None,
        });
        let velocity = schema::onvif::Ptzspeed { pan_tilt, zoom };
        let timeout: xsd_types::types::duration::Duration =
            xsd_types::types::duration::Duration::from_str("PT5S").unwrap();

        schema::ptz::continuous_move(
            ptz,
            &schema::ptz::ContinuousMove {
                profile_token,
                velocity,
                timeout: Some(timeout),
            },
        )
        .await
        .unwrap();
    }
}

async fn send_stop_ptz(device: &Device) {
    if let Some(ref ptz) = device.ptz {
        println!(
            "ptz stop: {:#?}",
            schema::ptz::stop(
                ptz,
                &schema::ptz::Stop {
                    profile_token: get_profile_token(device).await,
                    pan_tilt: Some(true),
                    zoom: Some(true)
                }
            )
            .await
            .unwrap()
        );
    }
}

async fn send_relative_ptz(device: &Device, pan: f64, tilt: f64, zoom: f64) {
    if let Some(ref ptz) = device.ptz {
        println!("relative pan: {}, tilt: {}, zoom: {}", pan, tilt, zoom);
        let space = Some("relative_pan_tilt_translation_space".to_string());
        let pan_tilt = Some(schema::common::Vector2D {
            x: pan,
            y: tilt,
            space,
        });
        let space = Some("relative_zoom_translation_space".to_string());
        let zoom = Some(schema::common::Vector1D { x: zoom, space });
        let translation = schema::onvif::Ptzvector { pan_tilt, zoom };
        let speed = None;

        println!(
            "ptz relative move: {:#?}",
            schema::ptz::relative_move(
                ptz,
                &schema::ptz::RelativeMove {
                    profile_token: get_profile_token(&device).await,
                    translation,
                    speed
                }
            )
            .await
            .unwrap()
        );
    }
}

fn translate_recenter(
    device: &Device,
    onvif_model: Option<String>,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
) {
    let pan = x as f64 / rect_width as f64;
    let tilt = -y as f64 / rect_height as f64;
    let zoom = 0.0;

    task::block_on(async {
        // if onvif_model
        //     .unwrap_or("".to_string())
        //     .eq_ignore_ascii_case(RELATIVE_BLACKLIST)
        // {
        send_continuous_ptz(device, pan, -tilt, zoom).await;
        let time = (500.0 * (pan * pan + tilt * tilt).sqrt()) as u64;
        async_std::task::sleep(std::time::Duration::from_millis(time)).await;
        send_stop_ptz(&device).await;
        // } else {
        //     send_relative_ptz(&device, pan, tilt, zoom).await;
        // }
    });
}

#[tokio::main]
async fn main() {
    let uri = Url::parse("http://192.168.1.15:888").unwrap();
    let device = Device::new(
        Some(uri),
        Some("test".to_owned()),
        Some("test123".to_owned()),
    )
    .unwrap();

    async_std::task::block_on(async {
        match schema::devicemgmt::get_capabilities(&device.device_mgmt, &Default::default()).await {
            Ok(capabilities) => println!("{:#?}", capabilities),
            Err(error) => println!("Failed to fetch capabilities: {}", error.to_string()),
        };

        match schema::devicemgmt::get_device_information(&device.device_mgmt, &Default::default())
            .await
        {
            Ok(info) => println!("{:#?}", info),
            Err(error) => println!("Failed to fetch information: {}", error.to_string()),
        }

        if let Some(ref ptz) = device.ptz {
            let config = schema::ptz::get_configurations(ptz, &schema::ptz::GetConfigurations {})
                .await
                .unwrap();
            println!("{:#?}", config);
        }

        // send_continuous_ptz(&device, -0.5, 0.0, 0.0).await;
        // send_relative_ptz(&device, 0.5, 0.0, 0.0).await;
    });
}
