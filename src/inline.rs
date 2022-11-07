use std::collections::{BTreeMap, HashMap};

use crate::{
  matches_pattern, minus,
  patterns::{find_at, PatMatch},
  plus, Match, Opts, Warn,
};

#[derive(Default)]
pub struct Parser {
  opts: Opts,
  warn: Option<Warn>,
  subject: String,
  matches: BTreeMap<usize, Match>,
  openers: HashMap<u8, Vec<Opener>>,
  verbatim: usize,
  verbatim_type: &'static str,
  destination: bool,
  firstpos: usize,
  lastpos: usize,
  allow_attributes: bool,
  attribute_parser: (),
  attribute_start: (),
  attribute_slices: (),
}

#[derive(Debug, Clone, Copy)]
struct Opener {
  spos: usize,
  epos: usize,
  annot: &'static str,
  subspos: usize,
  subepos: usize,
}

impl Opener {
  fn new(spos: usize, epos: usize) -> Self {
    Self { spos, epos, annot: "", subspos: 0, subepos: 0 }
  }
}

// allow up to 3 captures...
fn bounded_find<'a>(
  subj: &'a str,
  patt: &'static str,
  startpos: usize,
  endpos: usize,
) -> PatMatch<'static> {
  let mut m = find_at(subj, patt, startpos);
  if m.end > endpos {
    m = PatMatch::default()
  }
  m
}

impl Parser {
  pub fn new(subject: String, opts: Opts, warn: Option<Warn>) -> Parser {
    let mut res = Parser::default();
    res.subject = subject;
    res.opts = opts;
    res.warn = warn;
    res
  }

  fn add_match(&mut self, startpos: usize, endpos: usize, annotation: &'static str) {
    self.matches.insert(startpos, (startpos, endpos, annotation));
  }

  fn add_opener(&mut self, name: u8, opener: Opener) {
    self.openers.entry(name).or_default().push(opener)
  }

  fn clear_openers(&mut self, startpos: usize, endpos: usize) {
    for v in self.openers.values_mut() {
      v.retain(|it| !(startpos <= it.spos && it.epos <= endpos))
    }
  }

  fn str_matches(&mut self, startpos: usize, endpos: usize) {
    for i in startpos..endpos {
      if let Some(m) = self.matches.get_mut(&i) {
        if m.2 != "str" && m.2 != "escape" {
          m.2 = "str";
        }
      }
    }
  }

  fn between_matched(
    &mut self,
    pos: usize,
    c: u8,
    annotation: &'static str,
    defaultmatch: &'static str,
  ) -> usize {
    let mut can_open = find_at(&self.subject, "^%S", pos + 1).is_match;
    let mut _can_close = find_at(&self.subject, "^%S", pos - 1).is_match;
    let has_open_marker = matches_pattern(self.matches.get(&(pos - 1)), "open_marker");
    let hash_close_marker = self.subject.as_bytes()[pos + 1] == b'}';
    let mut endcloser = pos;
    let mut startopener = pos;

    // TODO: opentest

    // allow explicit open/close markers to override:
    if has_open_marker {
      _can_close = true;
      can_open = false;
      startopener = pos - 1;
    }
    if !has_open_marker && hash_close_marker {
      _can_close = true;
      can_open = false;
      endcloser = pos + 1;
    }

    // TODO: defaultmatch

    let openers = self.openers.entry(c).or_default();
    if _can_close && openers.len() > 0 {
      // check openers for a match
      let opener = *openers.last().unwrap();
      if opener.epos != pos - 1 {
        // exclude empty emph
        self.clear_openers(opener.spos, pos);
        self.add_match(opener.spos, opener.epos, plus(annotation));
        self.add_match(pos, endcloser, minus(annotation));
        return endcloser + 1;
      }
    }
    // if we get here, we didn't match an opener
    if can_open {
      self.add_opener(c, Opener::new(startopener, pos));
      self.add_match(startopener, pos + 1, defaultmatch);
      pos + 1
    } else {
      self.add_match(startopener, endcloser + 1, defaultmatch);
      endcloser + 1
    }
  }

  fn matchers(&mut self, c: u8, pos: usize, endpos: usize) -> Option<usize> {
    match c {
      b'`' => {
        let m = bounded_find(&self.subject, "^`*", pos, endpos);
        if !m.is_match {
          return None;
        }
        // TODO: display/inline math

        self.add_match(pos, m.end, "+verbatim");
        self.verbatim_type = "-verbatim";

        self.verbatim = m.end - pos;
        return Some(m.end);
      }
      b'\\' => {
        let m = bounded_find(&self.subject, "^[ \t]*\r?\n", pos + 1, endpos);
        self.add_match(pos, pos + 1, "escape");

        if m.is_match {
          // see f there were preceding spaces
          if let Some((_, &(sp, mut ep, annot))) = self.matches.iter().rev().next() {
            if annot == "str" {
              while self.subject.as_bytes()[ep] == b' ' || self.subject.as_bytes()[ep] == b'\t' {
                ep = ep - 1
              }
              if sp == ep {
                self.matches.remove(&sp);
              } else {
                self.add_match(sp, ep, "str")
              }
            }
          }
          self.add_match(pos + 1, m.end, "hardbreak");
          return Some(m.end);
        } else {
          let m = bounded_find(&self.subject, "^[%p ]", pos + 1, endpos);
          if !m.is_match {
            self.add_match(pos, pos + 1, "str");
            return Some(pos + 1);
          } else {
            self.add_match(pos, pos + 1, "escape");
            if find_at(&self.subject, "^ ", pos + 1).is_match {
              self.add_match(pos + 1, m.end, "nbsp")
            } else {
              self.add_match(pos + 1, m.end, "str")
            }
            return Some(m.end);
          }
        }
      }
      b'<' => {
        let url = bounded_find(&self.subject, "^%<[^<>%s]+%>", pos, endpos);
        if url.is_match {
          let is_url = bounded_find(&self.subject, "^%a+:", pos + 1, url.end).is_match;
          let is_email = bounded_find(&self.subject, "^[^:]+%@", pos + 1, url.end).is_match;
          if is_email {
            self.add_match(url.start, url.start + 1, "+email");
            self.add_match(url.start + 1, url.end - 1, "str");
            self.add_match(url.end - 1, url.end, "-email");
            return Some(url.end);
          } else if is_url {
            self.add_match(url.start, url.start + 1, "+url");
            self.add_match(url.start + 1, url.end - 1, "str");
            self.add_match(url.end - 1, url.end, "-url");
            return Some(url.end);
          }
        }
        return None;
      }
      b'~' => Some(self.between_matched(pos, b'~', "subscript", "str")),
      b'^' => Some(self.between_matched(pos, b'^', "superscript", "str")),
      b'[' => {
        let m = bounded_find(&self.subject, "^%^([^]]+)%]", pos + 1, endpos);
        if m.is_match {
          self.add_match(pos, m.end, "footnote_reference");
          return Some(m.end);
        } else {
          self.add_opener(b'[', Opener::new(pos, pos + 1));
          self.add_match(pos, pos + 1, "str");
          return Some(pos + 1);
        }
      }
      b']' => {
        let openers = self.openers.entry(b'[').or_default();
        if openers.len() > 0 {
          let opener = openers.last_mut().unwrap();
          if opener.annot == "reference_link" {
            let opener = *opener;
            // found a reference link
            // add the matches
            let is_image = self.subject[..opener.spos].ends_with('!')
              && !self.subject[..opener.spos].ends_with("[]");
            if is_image {
              self.add_match(opener.spos - 1, opener.spos, "image_marker");
              self.add_match(opener.spos, opener.epos, "+imagetext");
              self.add_match(opener.subspos, opener.subepos, "-imagetext");
            } else {
              self.add_match(opener.spos, opener.epos, "+linktext");
              self.add_match(opener.subspos, opener.subepos, "-linktext");
            }
            self.add_match(opener.subepos - 1, opener.subepos, "+reference");
            self.add_match(pos, pos, "-reference");
            // convert all matches to str
            self.str_matches(opener.subepos + 1, pos);
            // remove from openers
            self.clear_openers(opener.spos, pos);
            return Some(pos + 1);
          } else if bounded_find(&self.subject, "^[%[]", pos + 1, endpos).is_match {
            opener.annot = "reference_link";
            opener.subspos = pos; // intermediate ]
            opener.subepos = pos + 2; // intermediate [
            self.add_match(pos, pos + 2, "str");
            return Some(pos + 2);
          } else if bounded_find(&self.subject, "^[(]", pos + 1, endpos).is_match {
            opener.annot = "explicit_link";
            opener.subspos = pos; // intermediate ]
            opener.subepos = pos + 2; // intermediate (
            self.openers.remove(&b'('); // clear ( openers
            self.destination = true;
            self.add_match(pos, pos + 2, "str");
            return Some(pos + 2);
          }
        }
        return None;
      }
      b'(' => {
        if !self.destination {
          return None;
        }
        self.add_opener(b'(', Opener::new(pos, pos + 1));
        self.add_match(pos, pos + 1, "str");
        return Some(pos + 1);
      }
      b')' => {
        if !self.destination {
          return None;
        }
        let parens = self.openers.entry(b'(').or_default();
        if parens.len() > 0 {
          // TODO?
          parens.pop();
          self.add_match(pos, pos + 1, "str");
          return Some(pos + 1);
        } else {
          let openers = &self.openers.entry(b'[').or_default().clone();
          if let Some(&opener) = openers.last() {
            if opener.annot == "explicit_link" {
              let (startdest, enddest) = (opener.subepos - 1, pos);
              // we have inline link
              let is_image = self.subject[..opener.spos].ends_with('!')
                && !self.subject[..opener.spos].ends_with("[]");
              if is_image {
                self.add_match(opener.spos - 1, opener.spos, "image_marker");
                self.add_match(opener.spos, opener.epos, "+imagetext");
                self.add_match(opener.subspos, opener.subspos + 1, "-imagetext");
              } else {
                self.add_match(opener.spos, opener.epos, "+linktext");
                self.add_match(opener.subspos, opener.subspos + 1, "-linktext");
              }
              self.add_match(startdest, startdest + 1, "+destination");
              self.add_match(enddest, enddest + 1, "-destination");
              self.destination = false;
              // convert all matches to str
              self.str_matches(opener.subepos + 1, pos);
              // remove from openers
              self.clear_openers(opener.spos, pos);
              return Some(enddest + 1);
            }
          }
          return None;
        }
      }
      b'_' => Some(self.between_matched(pos, b'_', "emph", "str")),
      b'*' => Some(self.between_matched(pos, b'*', "strong", "str")),
      b'{' => todo!(),
      b':' => todo!(),
      b'+' => todo!(),
      b'=' => todo!(),
      b'\'' => todo!(),
      b'"' => todo!(),
      b'-' => todo!(),
      b'.' => {
        if bounded_find(&self.subject, "^%.%.", pos + 1, endpos).is_match {
          self.add_match(pos, pos + 3, "ellipses");
          return Some(pos + 3);
        }
        return None;
      }
      _ => return None,
    }
  }
  fn single_char(&mut self, pos: usize) -> usize {
    self.add_match(pos, pos + 1, "str");
    pos + 1
  }

  // Feed a slice to the parser, updating state.
  pub fn feed(&mut self, spos: usize, endpos: usize) {
    let special = "[%]%[\\`{}_*()!<>~^:=+$\r\n'\".-]";
    let subject = self.subject.clone();
    if self.firstpos == 0 || spos < self.firstpos {
      self.firstpos = spos
    }
    if self.lastpos == 0 || endpos > self.lastpos {
      self.lastpos = endpos
    }
    let mut pos = spos;
    while pos < endpos {
      if false {
        // TODO: attributes
      } else {
        // find next interesting character:
        let newpos = bounded_find(&subject, special, pos, endpos).or(endpos);
        if newpos > pos {
          self.add_match(pos, newpos, "str");
          pos = newpos;
          if pos > endpos {
            break; // otherwise, fall through:
          }
        }
        // if we get here, then newpos = pos,
        // i.e. we have something interesting at pos
        let c = subject.as_bytes()[pos];
        if c == b'\r' || c == b'\n' {
          if c == b'\r' && bounded_find(&subject, "^[%n]", pos + 1, endpos).is_match {
            self.add_match(pos, pos + 2, "softbreak");
            pos = pos + 2
          } else {
            self.add_match(pos, pos + 1, "softbreak");
            pos = pos + 1
          }
        } else if self.verbatim > 0 {
          if c == b'`' {
            let m = bounded_find(&subject, "^`+", pos, endpos);
            if m.is_match && m.end - pos == self.verbatim {
              // TODO: Check for raw attributes
              self.add_match(pos, m.end, self.verbatim_type);
              pos = m.end;
            } else {
              let endchar = m.end_or(endpos);
              self.add_match(pos, endchar, "str");
              pos = endchar
            }
          } else {
            self.add_match(pos, pos, "str");
            pos = pos + 1
          }
        } else {
          pos = self.matchers(c, pos, endpos).unwrap_or_else(|| self.single_char(pos))
        }
      }
    }
  }

  pub fn get_matches(&mut self) -> Vec<Match> {
    let mut sorted: Vec<Match> = Vec::new();
    let (mut lastsp, mut lastep, mut lastannot) = (0, 0, "");
    for i in self.firstpos..=self.lastpos {
      if let Some(&(sp, ep, annot)) = self.matches.get(&i) {
        if annot == "str" && lastannot == "str" && lastep == sp {
          (*sorted.last_mut().unwrap()).1 = ep;
          (lastsp, lastep, lastannot) = (lastsp, ep, annot)
        } else {
          sorted.push((sp, ep, annot));
          (lastsp, lastep, lastannot) = (sp, ep, annot)
        }
      }
    }
    if sorted.len() > 0 {
      // remove final softbreak
      if sorted.last().unwrap().2 == "softbreak" {
        sorted.pop();
      }
    }
    sorted
  }
}
