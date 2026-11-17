pub struct OrderHandler;

impl OrderHandler {
    pub fn create(req: Request) -> Response {
        let order = match parse_order(&req.body) {
            Ok(order) => order,
            Err(e) => return Response::bad_request(e),
        };
        match OrderService::place(order, Strictness::Strict) {
            Ok(saved) => Response::created(to_json(&saved)),
            Err(e) => Response::unprocessable(e),
        }
    }

    pub fn create_bulk(req: Request) -> Response {
        let orders = match parse_orders(&req.body) {
            Ok(orders) => orders,
            Err(e) => return Response::bad_request(e),
        };
        for order in orders {
            if let Err(e) = OrderService::place(order, Strictness::Strict) {
                return Response::unprocessable(e);
            }
        }
        Response::accepted()
    }
}

fn parse_order(body: &[u8]) -> Result<Order, ParseError> {
    serde_json::from_slice(body).map_err(ParseError::from)
}

fn parse_orders(body: &[u8]) -> Result<Vec<Order>, ParseError> {
    serde_json::from_slice(body).map_err(ParseError::from)
}
