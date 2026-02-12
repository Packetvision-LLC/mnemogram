use aws_sdk_dynamodb::Client as DynamoClient;
use chrono::{DateTime, Utc};
use lambda_http::{run, service_fn, Body, Error, Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use hmac::{Hmac, Mac};
use sha2::Sha256;

#[derive(Debug, Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    data: StripeEventData,
    created: i64,
}

#[derive(Debug, Deserialize)]
struct StripeEventData {
    object: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CheckoutSession {
    id: String,
    customer: Option<String>,
    subscription: Option<String>,
    metadata: Option<HashMap<String, String>>,
    total_details: Option<CheckoutTotalDetails>,
}

#[derive(Debug, Deserialize)]
struct CheckoutTotalDetails {
    amount_discount: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Subscription {
    id: String,
    customer: String,
    status: String,
    current_period_end: i64,
    current_period_start: i64,
    metadata: Option<HashMap<String, String>>,
    discount: Option<Discount>,
    items: SubscriptionItems,
}

#[derive(Debug, Deserialize)]
struct SubscriptionItems {
    data: Vec<SubscriptionItem>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionItem {
    price: Price,
}

#[derive(Debug, Deserialize)]
struct Price {
    id: String,
    metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct Discount {
    coupon: Option<Coupon>,
    promotion_code: Option<PromotionCode>,
}

#[derive(Debug, Deserialize)]
struct Coupon {
    id: String,
}

#[derive(Debug, Deserialize)]
struct PromotionCode {
    code: String,
}

#[derive(Debug, Deserialize)]
struct Invoice {
    id: String,
    customer: String,
    subscription: Option<String>,
    status: String,
    total: i64,
    currency: String,
    period_end: i64,
    period_start: i64,
}

fn verify_stripe_signature(
    payload: &str,
    signature_header: &str,
    webhook_secret: &str,
) -> Result<(), String> {
    let elements: HashMap<&str, &str> = signature_header
        .split(',')
        .filter_map(|element| {
            let mut parts = element.splitn(2, '=');
            Some((parts.next()?, parts.next()?))
        })
        .collect();

    let timestamp = elements.get("t").ok_or("Missing timestamp in signature")?;
    let signature = elements.get("v1").ok_or("Missing v1 signature in header")?;

    // Create the signed payload
    let signed_payload = format!("{}.{}", timestamp, payload);

    // Create HMAC
    let mut mac = Hmac::<Sha256>::new_from_slice(webhook_secret.as_bytes())
        .map_err(|e| format!("Invalid key: {}", e))?;
    mac.update(signed_payload.as_bytes());
    
    let expected_signature = hex::encode(mac.finalize().into_bytes());
    
    if signature == &expected_signature {
        Ok(())
    } else {
        Err("Signature verification failed".to_string())
    }
}

async fn handle_checkout_completed(
    dynamo: &DynamoClient,
    session: CheckoutSession,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing checkout.session.completed for session: {}", session.id);

    let user_id = session
        .metadata
        .as_ref()
        .and_then(|m| m.get("userId"))
        .ok_or("Missing userId in checkout session metadata")?;

    let customer_id = session
        .customer
        .as_ref()
        .ok_or("Missing customer in checkout session")?;

    let subscription_id = session
        .subscription
        .as_ref()
        .ok_or("Missing subscription in checkout session")?;

    // Extract promo code info if available
    let promo_code = if session.total_details
        .as_ref()
        .and_then(|td| td.amount_discount)
        .unwrap_or(0) > 0 {
        // If there was a discount, try to get the promo code from metadata
        session.metadata
            .as_ref()
            .and_then(|m| m.get("promoCode"))
            .map(|s| s.clone())
    } else {
        None
    };

    let now = Utc::now().to_rfc3339();
    
    let mut item = HashMap::new();
    item.insert("userId".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(user_id.clone()));
    item.insert("stripeCustomerId".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(customer_id.clone()));
    item.insert("stripeSubscriptionId".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(subscription_id.clone()));
    item.insert("status".to_string(), aws_sdk_dynamodb::types::AttributeValue::S("active".to_string()));
    item.insert("createdAt".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(now.clone()));
    item.insert("updatedAt".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(now));

    // Add promo code if present
    if let Some(promo) = promo_code {
        item.insert("promoCode".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(promo));
        info!("Stored promo code for user: {}", user_id);
    }

    dynamo
        .put_item()
        .table_name(std::env::var("SUBSCRIPTIONS_TABLE")?)
        .set_item(Some(item))
        .send()
        .await?;

    info!("Created subscription record for user: {}", user_id);
    Ok(())
}

async fn handle_subscription_updated(
    dynamo: &DynamoClient,
    subscription: Subscription,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing customer.subscription.updated for subscription: {}", subscription.id);

    let user_id = subscription
        .metadata
        .as_ref()
        .and_then(|m| m.get("userId"))
        .ok_or("Missing userId in subscription metadata")?;

    // Extract plan ID from subscription items
    let plan_id = subscription.items.data
        .first()
        .and_then(|item| item.price.metadata.as_ref())
        .and_then(|m| m.get("planId"))
        .map(|s| s.as_str())
        .unwrap_or("unknown");

    let current_period_end = DateTime::from_timestamp(subscription.current_period_end, 0)
        .unwrap_or_else(|| Utc::now())
        .to_rfc3339();

    let now = Utc::now().to_rfc3339();

    let mut update_expr = "SET #status = :status, #planId = :planId, #currentPeriodEnd = :currentPeriodEnd, #updatedAt = :updatedAt".to_string();
    let mut expr_attr_names = HashMap::new();
    let mut expr_attr_values = HashMap::new();

    expr_attr_names.insert("#status".to_string(), "status".to_string());
    expr_attr_names.insert("#planId".to_string(), "planId".to_string());
    expr_attr_names.insert("#currentPeriodEnd".to_string(), "currentPeriodEnd".to_string());
    expr_attr_names.insert("#updatedAt".to_string(), "updatedAt".to_string());

    expr_attr_values.insert(":status".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(subscription.status.clone()));
    expr_attr_values.insert(":planId".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(plan_id.to_string()));
    expr_attr_values.insert(":currentPeriodEnd".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(current_period_end));
    expr_attr_values.insert(":updatedAt".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(now));

    // Handle promo code from discount
    if let Some(discount) = &subscription.discount {
        if let Some(promotion_code) = &discount.promotion_code {
            update_expr.push_str(", #promoCode = :promoCode");
            expr_attr_names.insert("#promoCode".to_string(), "promoCode".to_string());
            expr_attr_values.insert(":promoCode".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(promotion_code.code.clone()));
            info!("Updated promo code for user: {}", user_id);
        }
    }

    dynamo
        .update_item()
        .table_name(std::env::var("SUBSCRIPTIONS_TABLE")?)
        .key("userId", aws_sdk_dynamodb::types::AttributeValue::S(user_id.clone()))
        .update_expression(update_expr)
        .set_expression_attribute_names(Some(expr_attr_names))
        .set_expression_attribute_values(Some(expr_attr_values))
        .send()
        .await?;

    info!("Updated subscription record for user: {}", user_id);
    Ok(())
}

async fn handle_subscription_deleted(
    dynamo: &DynamoClient,
    subscription: Subscription,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing customer.subscription.deleted for subscription: {}", subscription.id);

    let user_id = subscription
        .metadata
        .as_ref()
        .and_then(|m| m.get("userId"))
        .ok_or("Missing userId in subscription metadata")?;

    let now = Utc::now().to_rfc3339();

    let mut expr_attr_names = HashMap::new();
    let mut expr_attr_values = HashMap::new();

    expr_attr_names.insert("#status".to_string(), "status".to_string());
    expr_attr_names.insert("#updatedAt".to_string(), "updatedAt".to_string());

    expr_attr_values.insert(":status".to_string(), aws_sdk_dynamodb::types::AttributeValue::S("cancelled".to_string()));
    expr_attr_values.insert(":updatedAt".to_string(), aws_sdk_dynamodb::types::AttributeValue::S(now));

    dynamo
        .update_item()
        .table_name(std::env::var("SUBSCRIPTIONS_TABLE")?)
        .key("userId", aws_sdk_dynamodb::types::AttributeValue::S(user_id.clone()))
        .update_expression("SET #status = :status, #updatedAt = :updatedAt")
        .set_expression_attribute_names(Some(expr_attr_names))
        .set_expression_attribute_values(Some(expr_attr_values))
        .send()
        .await?;

    info!("Marked subscription as cancelled for user: {}", user_id);
    Ok(())
}

async fn handle_invoice_payment_succeeded(
    _dynamo: &DynamoClient,
    invoice: Invoice,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing invoice.payment_succeeded for invoice: {}", invoice.id);
    // Could implement payment success logic here (e.g., update payment status, extend access)
    Ok(())
}

async fn handle_invoice_payment_failed(
    dynamo: &DynamoClient,
    invoice: Invoice,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing invoice.payment_failed for invoice: {}", invoice.id);

    if let Some(subscription_id) = invoice.subscription {
        // Could implement logic to handle failed payments
        // For now, just log the event
        warn!("Payment failed for subscription: {} (customer: {})", subscription_id, invoice.customer);
    }

    Ok(())
}

async fn handler(event: Request) -> Result<Response<Body>, Error> {
    // Parse the Stripe webhook event
    let body = match event.body() {
        Body::Text(text) => text,
        Body::Binary(bytes) => std::str::from_utf8(bytes)?,
        Body::Empty => "",
    };

    // Verify webhook signature
    let webhook_secret = std::env::var("STRIPE_WEBHOOK_SECRET")
        .map_err(|_| "Missing STRIPE_WEBHOOK_SECRET environment variable")?;
    
    let signature_header = event
        .headers()
        .get("stripe-signature")
        .and_then(|h| h.to_str().ok())
        .ok_or("Missing Stripe-Signature header")?;

    if let Err(e) = verify_stripe_signature(body, signature_header, &webhook_secret) {
        error!("Webhook signature verification failed: {}", e);
        let body = json!({
            "status": "error",
            "error": "Invalid signature"
        });
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body)?))
            .map_err(Box::new)?);
    }

    let stripe_event: StripeEvent = serde_json::from_str(body)
        .map_err(|e| format!("Failed to parse Stripe event: {}", e))?;

    info!("Received Stripe event: {} ({})", stripe_event.event_type, stripe_event.id);

    let config = aws_config::load_from_env().await;
    let dynamo = DynamoClient::new(&config);

    let result = match stripe_event.event_type.as_str() {
        "checkout.session.completed" => {
            let session: CheckoutSession = serde_json::from_value(stripe_event.data.object)?;
            handle_checkout_completed(&dynamo, session).await
        }
        "customer.subscription.updated" => {
            let subscription: Subscription = serde_json::from_value(stripe_event.data.object)?;
            handle_subscription_updated(&dynamo, subscription).await
        }
        "customer.subscription.deleted" => {
            let subscription: Subscription = serde_json::from_value(stripe_event.data.object)?;
            handle_subscription_deleted(&dynamo, subscription).await
        }
        "invoice.payment_succeeded" => {
            let invoice: Invoice = serde_json::from_value(stripe_event.data.object)?;
            handle_invoice_payment_succeeded(&dynamo, invoice).await
        }
        "invoice.payment_failed" => {
            let invoice: Invoice = serde_json::from_value(stripe_event.data.object)?;
            handle_invoice_payment_failed(&dynamo, invoice).await
        }
        _ => {
            info!("Unhandled event type: {}", stripe_event.event_type);
            Ok(())
        }
    };

    match result {
        Ok(_) => {
            let body = json!({
                "status": "success",
                "eventId": stripe_event.id,
                "eventType": stripe_event.event_type
            });

            Ok(Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body)?))
                .map_err(Box::new)?)
        }
        Err(e) => {
            error!("Error processing webhook: {}", e);
            
            let body = json!({
                "status": "error",
                "error": e.to_string(),
                "eventId": stripe_event.id,
                "eventType": stripe_event.event_type
            });

            Ok(Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body)?))
                .map_err(Box::new)?)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}