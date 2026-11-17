pub fn handle(req: Request, router: &Router) -> Response {
    let req = match authenticate(req) {
        Ok(req) => req,
        Err(e) => return Response::unauthorized(e),
    };
    let req = match limit_body(req) {
        Ok(req) => req,
        Err(e) => return Response::too_large(e),
    };
    router.dispatch(req)
}

fn authenticate(req: Request) -> Result<Request, AuthError> {
    let token = req.header("Authorization").ok_or(AuthError::Missing)?;
    verify_token(token)?;
    Ok(req)
}

fn limit_body(req: Request) -> Result<Request, LimitError> {
    if req.body.len() > MAX_BODY {
        return Err(LimitError::TooLarge(req.body.len()));
    }
    Ok(req)
}
