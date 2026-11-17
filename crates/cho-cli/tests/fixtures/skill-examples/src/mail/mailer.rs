pub fn send_confirmation(id: OrderId) {
    let message = render("order-confirmed", id);
    MAIL_VENDOR.send(message).unwrap_or_else(|e| log_failure(id, e));
}
