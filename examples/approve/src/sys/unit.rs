use super::{Department, DeptId, DeptKind, Person, Role, UserId};

/// A unit.
#[derive(Debug, Clone)]
pub struct Unit {
    /// Unit name.
    pub name: String,
    /// All departments under the unit.
    pub departments: Vec<Department>,
    /// All people under the unit.
    pub people: Vec<Person>,
}

impl Unit {
    /// Creates an empty unit.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            departments: Vec::new(),
            people: Vec::new(),
        }
    }

    /// Adds a person.
    ///
    /// - `id`: supplied explicitly by the caller and must be unique.
    /// - Panics if the id already exists (could also return `Result`).
    pub fn add_person(
        &mut self,
        id: impl Into<UserId>,
        name: impl Into<String>,
        role: Role,
    ) -> UserId {
        let id = id.into();
        if self.people.iter().any(|p| p.id == id) {
            panic!("duplicate person id: {}", id);
        }

        self.people.push(Person {
            id: id.clone(),
            name: name.into(),
            role,
        });

        id
    }

    /// Adds a department.
    ///
    /// - `id`: supplied explicitly by the caller and must be unique.
    /// - `parent_id`: parent department id; pass `None` for a top-level department.
    /// - `manager_id`: department head id; may be `None` and set later.
    /// - Panics if the id already exists.
    fn add_department(
        &mut self,
        id: impl Into<DeptId>,
        name: impl Into<String>,
        kind: DeptKind,
        parent_id: Option<DeptId>,
        manager_id: Option<UserId>,
    ) -> DeptId {
        let id = id.into();
        if self.departments.iter().any(|d| d.id == id) {
            panic!("duplicate department id: {}", id);
        }

        self.departments.push(Department {
            id: id.clone(),
            name: name.into(),
            kind,
            parent_id,
            manager_id,
            member_ids: Vec::new(),
        });

        id
    }

    /// Adds a person to the given department.
    ///
    /// - Panics if the department does not exist.
    /// - A member already in the department is not added twice.
    fn add_member(&mut self, dept_id: impl Into<DeptId>, person_id: UserId) {
        let dept_id = dept_id.into();
        let dept = self
            .departments
            .iter_mut()
            .find(|d| d.id == dept_id)
            .unwrap_or_else(|| panic!("department not found: {dept_id}"));

        if !dept.member_ids.contains(&person_id) {
            dept.member_ids.push(person_id);
        }
    }

    pub fn person(&self, id: &str) -> Option<&Person> {
        self.people.iter().find(|p| p.id == id)
    }

    /// Looks up a department by kind.
    pub fn dept_by_kind(&self, kind: DeptKind) -> Option<&Department> {
        self.departments.iter().find(|d| d.kind == kind)
    }

    /// Builds the sample "Test Company" data.
    ///
    /// Structure:
    /// - Test Company
    ///   - Executive Office: 1 president, 2 vice presidents, 1 assistant (4 people)
    ///     - R&D Dept: 1 manager + 4 employees (5 people)
    ///     - QA Dept: 1 manager + 4 employees (5 people)
    ///     - Finance Dept: 1 manager + 1 employee (2 people)
    ///
    /// Note: the 5 people in R&D and QA each include the department manager.
    pub fn build() -> Self {
        let mut unit = Unit::new("Test Company");

        // ===== 1. Executive office =====
        // Executive office person ids start at 1.
        let president = unit.add_person("1", "Alice Zhang", Role::President);
        let vp1 = unit.add_person("2", "Bob Li", Role::VicePresident);
        let vp2 = unit.add_person("3", "Carol Wang", Role::VicePresident);
        let assistant = unit.add_person("4", "David Zhao", Role::Assistant);

        // The executive office is a top-level department; parent_id is None.
        // Its head is the president.
        let executive = unit.add_department(
            "1",
            "Executive Office",
            DeptKind::ExecutiveOffice,
            None,
            Some(president.clone()),
        );

        for pid in [president, vp1, vp2, assistant] {
            unit.add_member(executive.clone(), pid);
        }

        // ===== 2. R&D department =====
        // R&D person ids start at 101, to keep them apart from the executive office.
        let rnd_manager = unit.add_person("101", "Eve Chen", Role::DepartmentManager);
        let mut rnd_members = vec![rnd_manager.clone()];

        for i in 2..=4 {
            let id = unit.add_person(
                format!("10{}", i),
                format!("Rnd Employee {i}"),
                Role::Employee,
            );
            rnd_members.push(id);
        }

        // The R&D department reports to the executive office.
        let rnd = unit.add_department(
            "2",
            "R&D Department",
            DeptKind::Rnd,
            Some(executive.clone()),
            Some(rnd_manager.clone()),
        );

        for pid in rnd_members {
            unit.add_member(rnd.clone(), pid);
        }

        // ===== 3. QA department =====
        // QA person ids start at 201.
        let qa_manager = unit.add_person("201", "Frank Liu", Role::DepartmentManager);
        let mut qa_members = vec![qa_manager.clone()];

        for i in 2..=4 {
            let id = unit.add_person(
                format!("20{}", i),
                format!("QA Employee {i}"),
                Role::Employee,
            );
            qa_members.push(id);
        }

        // The QA department reports to the executive office.
        let qa = unit.add_department(
            "3",
            "QA Department",
            DeptKind::Qa,
            Some(executive.clone()),
            Some(qa_manager.clone()),
        );

        for pid in qa_members {
            unit.add_member(qa.clone(), pid);
        }

        // ===== 4. Finance department =====
        // Finance person ids start at 301.
        let finance_manager = unit.add_person("301", "Grace Sun", Role::DepartmentManager);
        let finance_member = unit.add_person("302", "Henry Zhou", Role::Employee);
        let finance_members = vec![finance_manager.clone(), finance_member];

        // The finance department reports to the executive office.
        let finance = unit.add_department(
            "4",
            "Finance Department",
            DeptKind::Finance,
            Some(executive.clone()),
            Some(finance_manager.clone()),
        );

        for pid in finance_members {
            unit.add_member(finance.clone(), pid);
        }

        unit
    }
}
