// Implements trusted-edit-inbound-object from uBlock Origin's wiki, Resources Library: "edit-inbound-object".
function zephiumTrustedEditInboundObject(chain = '', position = '', query = '') {
    zephiumInboundObject(chain, position, query, true);
}
