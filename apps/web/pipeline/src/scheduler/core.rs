use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MunicipalitySchedule {
    pub municipality_id: Uuid,
    pub already_scheduled_today: bool,
}

pub fn due_municipalities(schedules: impl IntoIterator<Item = MunicipalitySchedule>) -> Vec<Uuid> {
    schedules
        .into_iter()
        .filter(|schedule| !schedule.already_scheduled_today)
        .map(|schedule| schedule.municipality_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_unscheduled_municipalities_in_input_order() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let third = Uuid::new_v4();

        assert_eq!(
            due_municipalities([
                MunicipalitySchedule {
                    municipality_id: first,
                    already_scheduled_today: false
                },
                MunicipalitySchedule {
                    municipality_id: second,
                    already_scheduled_today: true
                },
                MunicipalitySchedule {
                    municipality_id: third,
                    already_scheduled_today: false
                },
            ]),
            vec![first, third]
        );
    }
}
