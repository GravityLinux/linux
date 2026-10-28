// SPDX-License-Identifier: GPL-2.0-only

//! Max parameter-memory-limit report and reply. Opaque report words never
//! supply ownership; the retained render and live descriptor do.
//! OOM reports contain fixed words (0,1), not pool/context at +4/+8.
//! Context is at +24; pool identity is the retained manager/descriptor graph.

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ParameterErrorG16C {
    kind: [u8;4],
    reserved: [u8;4],
    status: [u8;4],
    event: [u8;4],
    stamp: [u8;4],
    opaque_14: [u8;4],
    context: [u8;4],
    opaque_1c: [u8;12],
    descriptor: [u8;8],
    manager: [u8;8],
    queue: [u8;8],
    opaque_40: [u8;8],
}
#[repr(C)]
pub(crate) struct ParameterErrorReplyG16C {
    kind: [u8;4],
    success: [u8;4],
    queue: [u8;8],
    stamp: [u8;4],
    pad: [u8;44],
}
const _: () = {
    assert!(core::mem::size_of::<ParameterErrorG16C>()==0x48);
    assert!(core::mem::align_of::<ParameterErrorG16C>()==1);
    assert!(core::mem::offset_of!(ParameterErrorG16C,kind)==0);
    assert!(core::mem::offset_of!(ParameterErrorG16C,reserved)==4);
    assert!(core::mem::offset_of!(ParameterErrorG16C,status)==8);
    assert!(core::mem::offset_of!(ParameterErrorG16C,event)==12);
    assert!(core::mem::offset_of!(ParameterErrorG16C,stamp)==16);
    assert!(core::mem::offset_of!(ParameterErrorG16C,opaque_14)==20);
    assert!(core::mem::offset_of!(ParameterErrorG16C,context)==24);
    assert!(core::mem::offset_of!(ParameterErrorG16C,opaque_1c)==28);
    assert!(core::mem::offset_of!(ParameterErrorG16C,descriptor)==40);
    assert!(core::mem::offset_of!(ParameterErrorG16C,manager)==48);
    assert!(core::mem::offset_of!(ParameterErrorG16C,queue)==56);
    assert!(core::mem::offset_of!(ParameterErrorG16C,opaque_40)==64);
    assert!(core::mem::size_of::<ParameterErrorReplyG16C>()==64);
    assert!(core::mem::align_of::<ParameterErrorReplyG16C>()==1);
    assert!(core::mem::offset_of!(ParameterErrorReplyG16C,kind)==0);
    assert!(core::mem::offset_of!(ParameterErrorReplyG16C,success)==4);
    assert!(core::mem::offset_of!(ParameterErrorReplyG16C,queue)==8);
    assert!(core::mem::offset_of!(ParameterErrorReplyG16C,stamp)==16);
    assert!(core::mem::offset_of!(ParameterErrorReplyG16C,pad)==20);
};
impl Default for ParameterErrorReplyG16C {
    fn default()->Self {
        Self {kind:9u32.to_le_bytes(),success:1u32.to_le_bytes(),queue:[0;8],stamp:[0;4],pad:[0;44]}
    }
}
impl ParameterErrorReplyG16C {
    pub(crate) fn new(context:u32,queue:u64,stamp:u32)->Option<Self> {
        if !(1..64).contains(&context) || queue<0xfffffc0000000000 || queue&7!=0 {return None;}
        Some(Self {queue:queue.to_le_bytes(),
            stamp:stamp.to_le_bytes(),..Default::default()})
    }
    pub(crate) fn bytes(&self)->&[u8;64] {
        // SAFETY: Initialized byte arrays, no padding, exact layout above.
        unsafe { &*core::ptr::from_ref(self).cast::<[u8;64]>() }
    }
}
impl ParameterErrorG16C {
    pub(crate) fn parse(raw:&[u8;0x48])->Option<Self> {
        // SAFETY: Initialized byte arrays, alignment one, exact size above.
        let request=unsafe {raw.as_ptr().cast::<Self>().read()};
        (u32::from_le_bytes(request.kind)==7
            && u32::from_le_bytes(request.reserved)==0 && u32::from_le_bytes(request.status)==1
            && (1..64).contains(&request.context())).then_some(request)
    }
    pub(crate) fn context(&self)->u32 {u32::from_le_bytes(self.context)}
    pub(crate) fn event(&self)->u32 {u32::from_le_bytes(self.event)}
    pub(crate) fn stamp(&self)->u32 {u32::from_le_bytes(self.stamp)}
    pub(crate) fn reply(&self)->Option<ParameterErrorReplyG16C> {
        ParameterErrorReplyG16C::new(self.context(),u64::from_le_bytes(self.queue),self.stamp())
    }
}

/// Host publication identity, captured before exposing either producer.
#[derive(Clone, Copy)]
pub(crate) struct RenderOwnerG16C {
    pub(crate) context:u32,
    pub(crate) slot:u32,
    pub(crate) event:u32,
    pub(crate) stamp:u32,
    pub(crate) descriptor:u64,
    pub(crate) manager:u64,
    pub(crate) queue:u64,
    pub(crate) driver_stamps:[u64;2],
}
impl RenderOwnerG16C {
    pub(crate) fn matches_request(&self,request:&ParameterErrorG16C)->bool {
        request.context()==self.context
            && request.event()==self.event && request.stamp()==self.stamp
            && u64::from_le_bytes(request.descriptor)==self.descriptor
            && u64::from_le_bytes(request.manager)==self.manager
            && u64::from_le_bytes(request.queue)==self.queue
    }
    pub(crate) fn matches(&self,request:&ParameterErrorG16C,body:&[u8])->bool {
        if body.len()!=0x2240 {return false;}
        let word=|at|u32::from_le_bytes(body[at..at+4].try_into().unwrap());
        let pointer=|at|u64::from_le_bytes(body[at..at+8].try_into().unwrap());
        self.matches_request(request) && word(0)==1 && word(0xc)==self.context && pointer(0x28)==self.manager
            && pointer(0x2140)==self.driver_stamps[1]
            && word(0x2150)==self.stamp && word(0x2154)==self.event
    }
}
