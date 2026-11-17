pub fn routes(router: &mut Router) {
    router.post("/orders", OrderHandler::create);
    router.post("/orders/bulk", OrderHandler::create_bulk);
}

impl Router {
    pub fn dispatch(&self, req: Request) -> Response {
        match self.find(&req.method, &req.path) {
            Some(handler) => handler(req),
            None => Response::not_found(),
        }
    }
}
