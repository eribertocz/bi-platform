// Base roles of a project (ADR 0012 §4): defaults a project copies into its
// `policies/` directory and edits. Classification decides how far each role
// sees: public and internal objects are open to viewers, confidential ones
// are masked for them, restricted ones are masked for analysts and visible
// to modelers and administrators.

@id("viewer.view")
permit (
  principal in Role::"viewer",
  action in [Action::"view", Action::"use_in_dashboard"],
  resource
)
when { resource.classification == "public" || resource.classification == "internal" };

@id("viewer.masked")
permit (
  principal in Role::"viewer",
  action == Action::"view_masked",
  resource
)
when { resource.classification == "confidential" };

@id("analyst.view")
permit (
  principal in Role::"analyst",
  action in [Action::"view", Action::"use_in_dashboard"],
  resource
)
when { resource.classification != "restricted" };

@id("analyst.masked")
permit (
  principal in Role::"analyst",
  action == Action::"view_masked",
  resource
)
when { resource.classification == "restricted" };

@id("analyst.dashboards")
permit (
  principal in Role::"analyst",
  action in [Action::"edit", Action::"share"],
  resource
)
when { resource.classification != "restricted" };

@id("analyst.propose")
permit (
  principal in Role::"analyst",
  action == Action::"propose_change",
  resource
);

@id("modeler")
permit (
  principal in Role::"modeler",
  action in [
    Action::"view",
    Action::"use_in_dashboard",
    Action::"propose_change",
    Action::"approve",
    Action::"publish"
  ],
  resource
);

@id("admin")
permit (principal in Role::"admin", action, resource);
