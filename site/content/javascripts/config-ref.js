// Filters the config reference as you type: rows whose section, option,
// default or description don't contain every word are hidden, and so are
// sections left with no rows.
document$.subscribe(() => {
  const input = document.getElementById("config-filter");
  if (!input) return;
  const sections = [...document.querySelectorAll(".config-section")];

  const apply = () => {
    const words = input.value.toLowerCase().split(/\s+/).filter(Boolean);
    for (const row of document.querySelectorAll(".config-section tbody tr")) {
      const section = row.closest(".config-section").querySelector("h2, h3").textContent;
      const text = (section + " " + row.textContent).toLowerCase();
      row.classList.toggle("config-hidden", !words.every((w) => text.includes(w)));
    }
    for (const section of sections) {
      const shown = section.querySelector("tbody tr:not(.config-hidden)");
      section.classList.toggle("config-hidden", !shown);
    }
  };
  input.addEventListener("input", apply);
  apply();
});
