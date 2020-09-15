use std::fmt;

#[derive(PartialEq, Clone, Default)]
pub struct Address {
    pub data: std::vec::Vec<u8>,
    string: String,
}

impl Address {
    pub fn as_vec(&self) -> &Vec<u8> {
        &self.data
    }

    pub fn from_vec(vec: std::vec::Vec<u8>) -> Address {
        Address {
            string: bs58::encode(&vec).into_string(),
            data: vec,
        }
    }

    // we will not implement the trait because of the bs::decode::Error
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Address, bs58::decode::Error> {
        Ok(Address {
            string: String::from(s),
            data: bs58::decode(s).into_vec()?,
        })
    }

    pub fn as_str(&self) -> &str {
        self.string.as_str()
    }

    pub fn as_string(&self) -> &String {
        &self.string
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}