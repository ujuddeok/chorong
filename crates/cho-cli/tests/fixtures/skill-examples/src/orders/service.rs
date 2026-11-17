pub struct OrderService;

impl OrderService {
    pub fn place(order: Order, strictness: Strictness) -> Result<Order, OrderError> {
        validate(&order, strictness)?;
        let saved = insert(&order)?;
        QUEUE.push(Job::Confirm(saved.id));
        Ok(saved)
    }
}

fn validate(order: &Order, strictness: Strictness) -> Result<(), OrderError> {
    if order.lines.is_empty() {
        return Err(OrderError::Empty);
    }
    if strictness == Strictness::Strict && order.customer.is_none() {
        return Err(OrderError::NoCustomer);
    }
    Ok(())
}
