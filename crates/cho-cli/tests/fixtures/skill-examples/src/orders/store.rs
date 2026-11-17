pub fn insert(order: &Order) -> Result<Order, OrderError> {
    let id = DB.execute("INSERT INTO orders (customer, lines) VALUES (?, ?)", order.row())?;
    Ok(order.with_id(id))
}
