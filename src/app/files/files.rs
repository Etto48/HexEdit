#![allow(clippy::module_inception)]
use std::{error::Error, path::{Path, PathBuf}};

use ratatui::{backend::Backend, Terminal};

use crate::{app::{info_mode::InfoMode, notification::NotificationLevel, popup_state::PopupState, App}, headers::Header};

use super::path_result::PathResult;

impl App
{
    pub(in crate::app) fn go_to_path<B: Backend>(
        &mut self, 
        currently_open_path: &Path, 
        path: &str, 
        scroll: usize, 
        popup: &mut Option<PopupState>,
        terminal: &mut Terminal<B>
    ) -> Result<(), Box<dyn Error>>
    {
        let contents = Self::find_dir_contents(currently_open_path, path)?;
        if contents.is_empty()
        {
            return Err(format!("No files found that matches \"{}\"", path).into());
        }
        let selected = contents.into_iter().nth(scroll).expect("Scroll out of bounds for go_to_path.");

        if self.filesystem.is_dir(selected.path())
        {
            Self::open_dir(popup, &currently_open_path.join(selected.path()))?;
        }
        else
        {
            self.open_file(&selected.path().to_string_lossy(), Some(terminal))?;
            *popup = None;
        }
        
        Ok(())
    }

    pub(in crate::app) fn get_current_dir(&self) -> PathBuf
    {
        let current_path = self.filesystem.pwd();
        if self.filesystem.is_dir(current_path)
        {
            current_path.to_path_buf()
        }
        else 
        {
            current_path.parent().expect("A file should have a parent directory.").to_path_buf()
        }
    }

    pub(in crate::app) fn find_dir_contents(currently_open_path: &Path, path: &str) -> Result<Vec<PathResult>, Box<dyn Error>>
    {
        let mut ret = Vec::new();

        let input_path_as_path = Path::new(path);

        let (selected_dir, file_name) = if input_path_as_path.is_absolute()
        {
            if input_path_as_path.is_dir()
            {
                (input_path_as_path.canonicalize().expect("The path exists"), "".to_string())
            }
            else if let Some(parent) = input_path_as_path.parent()
            {
                if parent.is_dir()
                {
                    (parent.canonicalize().expect("The parent exists"), input_path_as_path.file_name().map_or("".into(), |name| name.to_string_lossy().to_string()))
                }
                else
                {
                    (currently_open_path.to_path_buf(), path.to_string())
                }
            }
            else
            {
                (currently_open_path.to_path_buf(), path.to_string())
            }
        }
        else
        {
            (currently_open_path.to_path_buf(), path.to_string())
        };
        

        let entries = std::fs::read_dir(&selected_dir)?;
        let mut entries = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path().strip_prefix(&selected_dir)
                .expect("The entry should be a child of the currently open path.").to_string_lossy().to_string())
            .collect::<Vec<_>>();


        if selected_dir.parent().is_some() && !input_path_as_path.is_absolute()
        {
            entries.insert(0, "..".into());
        }

        let entries = entries
            .into_iter()
            .filter(|entry| entry.to_lowercase().starts_with(&file_name.to_lowercase()));

        for entry in entries
        {
            let path = selected_dir.join(entry);
            if let Ok(path) = Self::path_canonicalize(&path, Some(&selected_dir))
            {
                ret.push(PathResult::new(&path, &selected_dir)?);
            }
        }

        Ok(ret)
    }

    pub(in crate::app) fn open_dir(popup: &mut Option<PopupState>, path: &Path) -> Result<(), Box<dyn Error>>
    {
        let path = dunce::canonicalize(path)?;
        *popup = Some(PopupState::Open { 
            currently_open_path: path.clone(),
            path: "".into(), 
            cursor: 0, 
            results: Self::find_dir_contents(&path, "")?, 
            scroll: 0 });
        Ok(())
    }

    pub fn log_header_info(&mut self)
    {
        if self.header != Header::None
        {
            match &self.header
            {
                Header::GenericHeader(header) => self.log(NotificationLevel::Info, &format!("File type: {:?}", header.file_type())),
                Header::None => unreachable!(),
            }
            self.log(NotificationLevel::Info, &format!("Architecture: {:?}", self.header.architecture()));
            self.log(NotificationLevel::Info, &format!("Bitness: {}", self.header.bitness()));
            self.log(NotificationLevel::Info, &format!("Entry point: {:#X}", self.header.entry_point()));
            for section in self.header.get_sections()
            {
                self.log(NotificationLevel::Info, &format!("Section: {}", section));
            }
        }
        else
        {
            self.log(NotificationLevel::Info, "No header found. Assuming 64-bit.");
        }
        
        self.log(NotificationLevel::Info, &format!("Press {} for a list of commands.", Self::key_event_to_string(self.settings.key.help)));
    }

    pub(in crate::app) fn open_file<B: Backend>(&mut self, path: &str, mut terminal: Option<&mut Terminal<B>>) -> Result<(), Box<dyn Error>>
    {
        self.log(NotificationLevel::Info, &format!("Opening file: \"{}\"", path));

        self.path = path.into();
        self.dirty = false;
        self.info_mode = InfoMode::Text;
        self.scroll = 0;
        self.cursor = (0,0);

        (self.screen_size, terminal) = if let Some(terminal) = terminal {(Self::get_size(terminal)?, Some(terminal))} else {((0,0), None)};
        self.block_size = 8;
        self.vertical_margin = 2;
        self.blocks_per_row = Self::calc_blocks_per_row(self.block_size, self.screen_size.0);

        terminal = if let Some(terminal) = terminal
        {
            Self::print_loading_status(&self.settings.color, &format!("Opening \"{}\"...", path), terminal)?;
            Some(terminal)
        } else {None};
        self.data = std::fs::read(&self.path)?;
        
        terminal = if let Some(terminal) = terminal
        {
            Self::print_loading_status(&self.settings.color, "Decoding binary data...", terminal)?;
            Some(terminal)
        } else {None};
        self.header = Header::parse_header(&self.data);

        terminal = if let Some(terminal) = terminal
        {
            Self::print_loading_status(&self.settings.color, "Disassembling executable...", terminal)?;
            Some(terminal)
        } else {None};
        (self.assembly_offsets, self.assembly_instructions) = Self::sections_from_bytes(&self.data, &self.header);

        if let Some(terminal) = terminal
        {
            Self::print_loading_status(&self.settings.color, "Opening ui...", terminal)?;
        }
        self.log_header_info();

        Ok(())
    }

    pub(in crate::app) fn path_canonicalize(path: &Path, base_path: Option<&Path>) -> Result<PathBuf, Box<dyn Error>>
    {
        let path_res = path.canonicalize();
        let mut path = match path_res
        {
            Ok(path) => path,
            Err(_) => {
                return Err(format!("Failed to canonicalize path \"{}\"", path.to_string_lossy()).into());
            }
        };
        if let Some(base_path) = base_path
        {
            let base_path = base_path.canonicalize()?;
            path = pathdiff::diff_paths(&path, base_path).expect("Failed to get relative path.");
        }
        Ok(path)
    }
}