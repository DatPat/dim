import { useCallback, useEffect } from "react";
import { useDispatch, useSelector } from "react-redux";
import { addNotification } from "slices/notifications";
import { formatHHMMSSDate } from "../../../Helpers/utils";
import {
  fetchInvites,
  createNewInvite,
  delInvite,
} from "../../../actions/auth.js";
import TrashIcon from "../../../assets/Icons/Trash";

function ManageInvites() {
  const dispatch = useDispatch();

  const { user, auth } = useSelector((store) => ({
    user: store.user,
    auth: store.auth,
  }));

  useEffect(() => {
    dispatch(fetchInvites());
  }, [auth.admin_exists, dispatch]);

  const genNewToken = useCallback(async () => {
    await dispatch(createNewInvite());
    dispatch(fetchInvites());
  }, [dispatch]);

  const delInviteToken = useCallback(
    async (token) => {
      await dispatch(delInvite(token));
      dispatch(fetchInvites());
    },
    [dispatch]
  );

  const toClipboard = (token) => {
    const text = token;
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(
        () => {
          dispatch(
            addNotification({ msg: "Token copied to clipboard." })
          );
        },
        () => {
          dispatch(
            addNotification({ msg: "Failed to copy token to clipboard." })
          );
        }
      );
    } else {
      const textarea = document.createElement("textarea");
      textarea.value = text;
      textarea.style.position = "fixed";
      textarea.style.opacity = "0";
      document.body.appendChild(textarea);
      textarea.select();
      try {
        document.execCommand("copy");
        dispatch(addNotification({ msg: "Token copied to clipboard." }));
      } catch {
        dispatch(
          addNotification({ msg: "Failed to copy token to clipboard." })
        );
      }
      document.body.removeChild(textarea);
    }
  };

  const tokens = auth.invites.items.map((token, i) => {
    const { hours, mins, secs, date, month, year } = formatHHMMSSDate(
      token.created
    );

    return (
      <div className="token" key={i} onClick={() => toClipboard(token.id)}>
        <p>{token.id}</p>
        <p>
          {hours}:{mins}:{secs} on the {date}/{month}/{year}
        </p>
        {token.claimed_by ? (
          <p>Claimed by {token.claimed_by}</p>
        ) : (
          <p>Available</p>
        )}
        {user.info.username !== token.claimed_by && (
          <button onClick={(e) => { e.stopPropagation(); delInviteToken(token.id); }}>
            <TrashIcon />
          </button>
        )}
      </div>
    );
  });

  return (
    <section>
      <h2>Manage invites</h2>
      <p className="desc">
        Create a token to invite someone and allow them access to your added
        media libraries securely.
      </p>
      <h3>Tokens</h3>
      <div className="tokensContainer">
        <div className="heading">
          <p>ID</p>
          <p>Created at</p>
          <p>Status</p>
        </div>
        <div className="separator" />
        <div className="tokens">{tokens}</div>
      </div>
      <button className="genTokenBtn" onClick={genNewToken}>
        Generate a new token
      </button>
    </section>
  );
}

export default ManageInvites;
