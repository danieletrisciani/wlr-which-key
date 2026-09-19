use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::str::FromStr;

use anyhow::{Error, Result, bail};
use pangocairo::{cairo, pango};
use wayrs_utils::keyboard::xkb;

use crate::DEBUG_LAYOUT;
use crate::color::Color;
use crate::config::{self, Config};
use crate::key::{Key, ModifierState, SingleKey};
use crate::text::{self, ComputedText};

/// Horizontal padding of the highlight around an item.
const PAD_LEFT: f64 = 10.0;
const PAD_RIGHT: f64 = 4.0;

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, width: f64, height: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.arc(x + width - r, y + r, r, 3.0 * FRAC_PI_2, TAU);
    cr.arc(x + width - r, y + height - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + height - r, r, FRAC_PI_2, PI);
    cr.close_path();
}

pub struct Menu {
    pages: Vec<MenuPage>,
    cur_page: usize,
    separator: ComputedText,
    /// Item whose key is currently held down, as `(page, column, item)`.
    pressed: Option<(usize, usize, usize)>,
}

struct MenuPage {
    item_height: f64,
    columns: Vec<MenuColumn>,
    /// Items that respond to their keys but are not drawn.
    hidden_items: Vec<MenuItem>,
    parent: Option<usize>,
}

struct MenuColumn {
    key_col_width: f64,
    val_col_width: f64,
    items: Vec<MenuItem>,
}

struct MenuItem {
    action: Action,
    key_comp: ComputedText,
    val_comp: ComputedText,
    key: Key,
}

#[derive(Clone)]
pub enum Action {
    Quit,
    Exec { cmd: String, keep_open: bool },
    Submenu(usize),
}

impl Menu {
    pub fn new(config: &Config) -> Result<Self> {
        let context = pango::Context::new();
        let fontmap = pangocairo::FontMap::new();
        context.set_font_map(Some(&fontmap));

        let mut this = Self {
            pages: Vec::new(),
            cur_page: 0,
            pressed: None,
            separator: ComputedText::new(&config.separator, &context, &config.font.0),
        };

        this.push_page(&context, &config.menu, config, None)?;

        Ok(this)
    }

    fn push_page(
        &mut self,
        context: &pango::Context,
        entries: &[config::Entry],
        config: &Config,
        parent: Option<usize>,
    ) -> Result<usize> {
        if entries.is_empty() {
            bail!("Empty menu pages are not allowed");
        }

        let cur_page = self.pages.len();

        self.pages.push(MenuPage {
            item_height: self.separator.height,
            columns: Vec::new(),
            hidden_items: Vec::new(),
            parent,
        });

        let mut visible_i = 0;
        for entry in entries {
            let (item, hidden) = match entry {
                config::Entry::Cmd {
                    key,
                    cmd,
                    desc,
                    keep_open,
                    hidden,
                } => (
                    MenuItem {
                        action: Action::Exec {
                            cmd: cmd.into(),
                            keep_open: *keep_open,
                        },
                        key_comp: ComputedText::new(key.to_string(), context, &config.font.0),
                        val_comp: ComputedText::new(desc, context, &config.font.0),
                        key: key.clone(),
                    },
                    *hidden,
                ),
                config::Entry::Recursive {
                    key,
                    submenu: entries,
                    desc,
                    hidden,
                } => {
                    let new_page = self.push_page(context, entries, config, Some(cur_page))?;
                    (
                        MenuItem {
                            action: Action::Submenu(new_page),
                            key_comp: ComputedText::new(key.to_string(), context, &config.font.0),
                            val_comp: ComputedText::new(
                                format!("+{desc}"),
                                context,
                                &config.font.0,
                            ),
                            key: key.clone(),
                        },
                        *hidden,
                    )
                }
            };

            if hidden {
                self.pages[cur_page].hidden_items.push(item);
                continue;
            }

            let height = f64::max(item.key_comp.height, item.val_comp.height);
            if height > self.pages[cur_page].item_height {
                self.pages[cur_page].item_height = height;
            }

            let col_i = config
                .rows_per_column
                .map_or(0, |rows_per_column| visible_i / rows_per_column);
            visible_i += 1;

            if col_i == self.pages[cur_page].columns.len() {
                self.pages[cur_page].columns.push(MenuColumn {
                    key_col_width: item.key_comp.width,
                    val_col_width: item.val_comp.width,
                    items: vec![item],
                });
            } else {
                let col = &mut self.pages[cur_page].columns[col_i];
                col.key_col_width = col.key_col_width.max(item.key_comp.width);
                col.val_col_width = col.val_col_width.max(item.val_comp.width);
                col.items.push(item);
            }
        }

        if self.pages[cur_page].columns.is_empty() {
            bail!("Menu pages must have at least one entry that is not hidden");
        }

        Ok(cur_page)
    }

    pub fn width(&self, config: &Config) -> f64 {
        let page = &self.pages[self.cur_page];
        page.columns
            .iter()
            .map(|col| col.key_col_width + col.val_col_width + self.separator.width)
            .sum::<f64>()
            + (page.columns.len() - 1) as f64 * config.column_padding()
            + (config.padding() + config.border_width) * 2.0
    }

    pub fn height(&self, config: &Config) -> f64 {
        let page = &self.pages[self.cur_page];
        page.columns
            .iter()
            .map(|col| page.item_height * col.items.len() as f64)
            .max_by(f64::total_cmp)
            .unwrap()
            + (config.padding() + config.border_width) * 2.0
    }

    pub fn render(&self, config: &config::Config, cairo_ctx: &cairo::Context) -> Result<()> {
        let mut dx = config.padding() + config.border_width;
        let dy = config.padding() + config.border_width;
        let page = &self.pages[self.cur_page];
        for (col_i, col) in page.columns.iter().enumerate() {
            self.render_column(config, cairo_ctx, dx, dy, page, col_i)?;
            dx += col.key_col_width
                + col.val_col_width
                + self.separator.width
                + config.column_padding();
        }
        Ok(())
    }

    fn render_column(
        &self,
        config: &Config,
        cairo_ctx: &cairo::Context,
        dx: f64,
        dy: f64,
        page: &MenuPage,
        col_i: usize,
    ) -> Result<()> {
        let column = &page.columns[col_i];
        for (i, comp) in column.items.iter().enumerate() {
            if config.highlight && self.pressed == Some((self.cur_page, col_i, i)) {
                let width = column.key_col_width
                    + self.separator.width
                    + column.val_col_width
                    + PAD_LEFT
                    + PAD_RIGHT;
                rounded_rect(
                    cairo_ctx,
                    dx - PAD_LEFT,
                    dy + page.item_height * (i as f64),
                    width,
                    page.item_height,
                    (page.item_height / 4.5).min(config.corner_r),
                );
                config.highlight_color.apply(cairo_ctx);
                cairo_ctx.fill()?;
            }

            comp.key_comp.render(
                cairo_ctx,
                text::RenderOptions {
                    x: dx + column.key_col_width - comp.key_comp.width,
                    y: dy + page.item_height * (i as f64),
                    fg_color: config.color,
                    height: page.item_height,
                },
            )?;
            self.separator.render(
                cairo_ctx,
                text::RenderOptions {
                    x: dx + column.key_col_width,
                    y: dy + page.item_height * (i as f64),
                    fg_color: config.color,
                    height: page.item_height,
                },
            )?;
            comp.val_comp.render(
                cairo_ctx,
                text::RenderOptions {
                    x: dx + column.key_col_width + self.separator.width,
                    y: dy + page.item_height * (i as f64),
                    fg_color: config.color,
                    height: page.item_height,
                },
            )?;
        }

        if *DEBUG_LAYOUT {
            Color::from_rgba(0, 0, 255, 255).apply(cairo_ctx);
            cairo_ctx.rectangle(
                dx,
                dy,
                column.key_col_width + column.val_col_width + self.separator.width,
                column.items.len() as f64 * page.item_height,
            );
            cairo_ctx.set_line_width(1.0);
            cairo_ctx.stroke().unwrap();
        }

        Ok(())
    }

    /// Position of the visible item bound to this key, if any.
    fn find_visible(&self, modifiers: ModifierState, sym: xkb::Keysym) -> Option<(usize, usize)> {
        let page = &self.pages[self.cur_page];
        page.columns.iter().enumerate().find_map(|(col_i, col)| {
            col.items
                .iter()
                .position(|i| i.key.matches(sym, modifiers))
                .map(|item_i| (col_i, item_i))
        })
    }

    /// Like [`Self::get_action`], but also highlights the item until [`Self::release`].
    pub fn press(&mut self, modifiers: ModifierState, sym: xkb::Keysym) -> Option<Action> {
        let action = self.get_action(modifiers, sym)?;
        self.pressed = self
            .find_visible(modifiers, sym)
            .map(|(col_i, item_i)| (self.cur_page, col_i, item_i));
        Some(action)
    }

    pub fn release(&mut self) {
        self.pressed = None;
    }

    pub fn get_action(&self, modifiers: ModifierState, sym: xkb::Keysym) -> Option<Action> {
        let page = &self.pages[self.cur_page];

        let action = page
            .columns
            .iter()
            .flat_map(|col| &col.items)
            .chain(&page.hidden_items)
            .find_map(|i| i.key.matches(sym, modifiers).then(|| i.action.clone()));
        if action.is_some() {
            return action;
        }

        match sym {
            xkb::Keysym::Escape => {
                return Some(Action::Quit);
            }
            xkb::Keysym::bracketleft | xkb::Keysym::g if modifiers.mod_ctrl => {
                return Some(Action::Quit);
            }
            xkb::Keysym::BackSpace => {
                if let Some(parent) = page.parent {
                    return Some(Action::Submenu(parent));
                }
            }
            _ => (),
        }

        None
    }

    pub fn set_page(&mut self, page: usize) {
        self.cur_page = page;
    }

    pub fn navigate_to_key_sequence(&mut self, key_sequence: &str) -> Result<Option<Action>> {
        let mut last_action = None;
        for key_str in key_sequence.split_whitespace() {
            if let Some((last_key_str, _action)) = &last_action {
                bail!("Key '{last_key_str}' leads to a command, but more keys follow in sequence");
            }
            let key = SingleKey::from_str(key_str).map_err(Error::msg)?;
            match self.get_action(key.modifiers, key.keysym) {
                Some(Action::Submenu(submenu_page)) => self.set_page(submenu_page),
                Some(action) => last_action = Some((key_str, action)),
                None => bail!("Key '{}' not found in current menu", key_str),
            }
        }
        Ok(last_action.map(|x| x.1))
    }
}
