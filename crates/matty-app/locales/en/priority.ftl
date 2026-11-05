## Message priority and acknowledgements (priority.rs)

priority-standard = Standard
priority-important = Important
priority-urgent = Urgent
priority-heading = Message priority
priority-request-ack = Request acknowledgement
priority-send-persistent = Send persistent notifications
# Parts of the composer's priority tag, joined with " · ".
priority-ack-requested = Acknowledgement requested
priority-persistent = Persistent notifications
# Stands in for the viewer's own name among who acknowledged.
priority-you = You
priority-no-acks = No one has acknowledged yet
priority-acked-by-one = Acknowledged by { $first }
priority-acked-by-two = Acknowledged by { $first } and { $second }
priority-acked-by-three = Acknowledged by { $first }, { $second } and { $third }
priority-acked-by-more = Acknowledged by { $first }, { $second } and { $count ->
        [one] { $count } other
       *[other] { $count } others
    }
priority-acknowledge = Acknowledge
priority-acknowledged = Acknowledged
priority-acknowledge-tip = Let the author know you saw this
priority-remove-ack = Remove acknowledgement
