pub struct SoapEndpoint;

impl SoapEndpoint {
    pub fn place_order(envelope: Envelope) -> Envelope {
        let order = match from_envelope(&envelope) {
            Ok(order) => order,
            Err(e) => return Envelope::fault(e),
        };
        match OrderService::place(order, Strictness::Lenient) {
            Ok(saved) => to_envelope(&saved),
            Err(e) => Envelope::fault(e),
        }
    }
}

fn from_envelope(envelope: &Envelope) -> Result<Order, ParseError> {
    let body = envelope.body("PlaceOrder")?;
    Order::from_xml(body)
}

fn to_envelope(order: &Order) -> Envelope {
    Envelope::new("PlaceOrderResponse", order.to_xml())
}
