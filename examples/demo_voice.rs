use std::env;
use std::fs;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use hound::{SampleFormat, WavSpec, WavWriter};
use main_error::MainError;
use steam_audio_codec::{SteamVoiceData, SteamVoiceDecoder};
use tf_demo_parser::demo::parser::MessageHandler;
use tf_demo_parser::MessageType;
pub use tf_demo_parser::{Demo, DemoParser, Parse, ParserState};
use tf_demo_parser::demo::data::DemoTick;
use tf_demo_parser::demo::message::Message;
use tf_demo_parser::demo::message::voice::{VoiceInitMessage};

fn main() -> Result<(), MainError> {
    let args: Vec<_> = env::args().collect();
    if args.len() < 2 {
        println!("1 argument required");
        return Ok(());
    }
    let path = args[1].clone();
    let file = fs::read(path)?;
    let demo = Demo::new(&file);
    let parser = DemoParser::new_with_analyser(demo.get_stream(), Voice::new("out.wav")?);
    let (_header, _writer) = parser.parse()?;

    Ok(())
}

struct Voice {
    out_buffer: Vec<i16>,
    writer: WavWriter<BufWriter<File>>,
    last_init: Option<VoiceInitMessage>,
    decoder: SteamVoiceDecoder,
}

impl Voice {
    fn new<P: AsRef<Path>>(path: P) -> Result<Voice, MainError> {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 24000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        Ok(Voice {
            out_buffer: vec![0; 8192],
            writer: WavWriter::create(path, spec)?,
            last_init: None,
            decoder: SteamVoiceDecoder::new(),
        })
    }
}

impl MessageHandler for Voice {
    type Output = WavWriter<BufWriter<File>>;

    fn does_handle(message_type: MessageType) -> bool {
        matches!(
            message_type,
            MessageType::VoiceInit | MessageType::VoiceData
        )
    }

    fn handle_message(&mut self, message: &Message, _tick: DemoTick, _parser_state: &ParserState) {
        match message {
            Message::VoiceInit(init) => {
                self.last_init = Some(init.clone());
            }
            Message::VoiceData(data) => {
                if let Some(init) = &self.last_init {
                    match init.codec.as_str() {
                        "steam" => {
                            let data = data.data.clone().read_bytes(data.length as usize / 8).unwrap();
                            let steam_data = SteamVoiceData::new(&data).unwrap();
                            let count = self.decoder.decode(steam_data, &mut self.out_buffer).unwrap();
                            for &sample in &self.out_buffer[0..count] {
                                self.writer.write_sample(sample).unwrap();
                            }
                        },
                        _ => panic!("this example only supports the steam voice codec")
                    };
                }
            }
            _ => {}
        }
    }

    fn into_output(self, _state: &ParserState) -> Self::Output {
        self.writer
    }
}

