use super::*;

#[derive(Parser)]
pub(crate) struct Extractor {
  #[clap(long)]
  batch_size: usize,
}

#[derive(Debug)]
struct Page {
  pub(crate) number: usize,
  pub(crate) url: String,
}

impl Page {
  pub(crate) fn content(&self) -> Result<String> {
    Ok(reqwest::blocking::get(self.url.clone())?.text()?)
  }
}

#[derive(Debug, Clone)]
struct Entry {
  pub(crate) department: String,
  pub(crate) faculty: String,
  pub(crate) level: String,
  pub(crate) terms: Vec<String>,
  pub(crate) url: String,
}

impl Entry {
  pub(crate) fn content(&self) -> Result<String> {
    Ok(reqwest::blocking::get(self.url.clone())?.text()?)
  }
}

impl Extractor {
  pub(crate) fn run(&self, source: PathBuf) -> Result {
    log::info!("Running loader...");

    let mut courses = Vec::new();

    let mut page = 0;

    while let Some(entries) = self.pages(self.aggregate(page, page + self.batch_size))? {
      courses.extend(
        entries
          .par_iter()
          .map(|entry| self.course(entry.clone()))
          .collect::<Result<Vec<Course>, _>>()?,
      );
      page += self.batch_size;
    }

    fs::write(source, serde_json::to_string(&courses)?).map_err(anyhow::Error::from)
  }

  fn aggregate(&self, start: usize, end: usize) -> Vec<Page> {
    (start..=end)
      .map(|index| Page {
        number: index,
        url: format!("{}/study/2022-2023/courses/search?page={}", BASE_URL, index),
      })
      .collect()
  }

  fn pages(&self, pages: Vec<Page>) -> Result<Option<Vec<Entry>>> {
    Ok(
      pages
        .par_iter()
        .map(|page| {
          self
            .page(page)
            .unwrap_or(Some(Vec::new()))
            .unwrap_or(Vec::new())
        })
        .flatten()
        .collect::<Vec<Entry>>()
        .into_option(),
    )
  }

  fn page(&self, page: &Page) -> Result<Option<Vec<Entry>>> {
    log::info!("Parsing html on page: {}...", page.number);

    let html = Html::parse_fragment(&page.content()?);

    let content = html
      .root_element()
      .select_optional("div[class='view-content']")?;

    if content.is_none() {
      log::info!("Did not find any content on page {}", page.number);
      return Ok(None);
    }

    log::info!("Parsing found content on page {}...", page.number);

    let results = content
      .unwrap()
      .select_many("div[class~='views-row']")?
      .iter()
      .map(|entry| -> Result<Entry> {
        Ok(Entry {
          department: entry
            .select_single("span[class~='views-field-field-dept-code']")?
            .select_single("span[class='field-content']")?
            .inner_html(),
          faculty: entry
            .select_single("span[class~='views-field-field-faculty-code']")?
            .select_single("span[class='field-content']")?
            .inner_html(),
          level: entry
            .select_single("span[class~='views-field-level']")?
            .select_single("span[class='field-content']")?
            .inner_html(),
          terms: entry
            .select_single("span[class~='views-field-terms']")?
            .select_single("span[class='field-content']")?
            .inner_html()
            .split(", ")
            .map(|term| term.to_owned())
            .collect::<Vec<String>>(),
          url: format!(
            "{}{}",
            BASE_URL,
            entry
              .select_single("div[class~='views-field-field-course-title-long']",)?
              .select_single("a")?
              .value()
              .attr("href")
              .ok_or_else(|| anyhow!("Failed to get attribute"))?
          ),
        })
      })
      .collect::<Result<Vec<Entry>, _>>();

    let entries = results?
      .into_iter()
      .filter(|entry| !entry.terms.contains(&String::from("Not Offered")))
      .collect::<Vec<Entry>>();

    log::info!("Scraped entries on page {}: {:?}", page.number, entries);

    Ok(Some(entries))
  }

  fn instructors(&self, input: &str) -> Vec<Instructor> {
    let mut ret = Vec::new();

    if input.contains("There are no professors associated with this course") {
      return ret;
    }

    let mut tokens = input.to_owned();

    ["Fall", "Winter", "Summer"].iter().for_each(|term| {
      match tokens.contains(&format!("({term})")) {
        false => return,
        _ => {
          let split = tokens.split(&format!("({term})")).collect::<Vec<&str>>();

          ret.extend(split[0].split(";").map(|s| {
            let curr = s.trim().split(", ").collect::<Vec<&str>>();
            Instructor {
              name: format!(
                "{} {}",
                curr.get(1).unwrap_or(&""),
                curr.get(0).unwrap_or(&"")
              ),
              term: term.to_string(),
            }
          }));

          if split.len() > 1 {
            tokens = split[1].trim().to_string();
          }
        }
      }
    });

    ret
  }

  fn course(&self, entry: Entry) -> Result<Course> {
    let html = Html::parse_fragment(&entry.content()?);

    let full_title = html
      .root_element()
      .select_single("h1[id='page-title']")?
      .inner_html()
      .trim()
      .to_owned();

    let full_code = full_title
      .split(' ')
      .take(2)
      .collect::<Vec<&str>>()
      .join(" ");

    let subject = full_code
      .split(' ')
      .take(1)
      .collect::<Vec<&str>>()
      .join(" ");

    let code = full_code
      .split(' ')
      .skip(1)
      .collect::<Vec<&str>>()
      .join(" ");

    let content = html
      .root_element()
      .select_single("div[class='node node-catalog clearfix']")?;

    let instructors = content
      .select_single("p[class='catalog-instructors']")?
      .inner_html()
      .trim()
      .split(' ')
      .skip(1)
      .collect::<Vec<&str>>()
      .join(" ")
      .trim()
      .to_owned();

    log::info!("Parsed course {}{}", subject, code);

    Ok(Course {
      id: Uuid::new_v5(
        &Uuid::NAMESPACE_X500,
        format!("{}-{}", subject, code).as_bytes(),
      )
      .to_string(),
      title: full_title
        .split(' ')
        .skip(2)
        .collect::<Vec<&str>>()
        .join(" "),
      subject,
      code,
      level: entry.level,
      url: entry.url,
      department: entry.department,
      faculty: entry.faculty,
      faculty_url: format!(
        "{}{}",
        BASE_URL,
        content
          .select_single("div[class='meta']")?
          .select_single("p")?
          .select_single("a")?
          .value()
          .attr("href")
          .ok_or_else(|| anyhow!("Failed to get attribute"))?
      ),
      description: content
        .select_single("div[class='content']")?
        .select_single("p")?
        .inner_html()
        .trim()
        .split(':')
        .skip(1)
        .collect::<Vec<&str>>()
        .join(" ")
        .trim()
        .to_owned(),
      terms: entry.terms,
      instructors: self.instructors(&instructors),
    })
  }
}
